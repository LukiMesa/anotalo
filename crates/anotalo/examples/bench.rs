//! Bench de la spec §1: velocidad y WER de whisper.cpp en proceso con los 60 clips en español que
//! prepara scripts/bench/prep_clips.py, comparados con la marca de bench/bar.json.
//!
//! cargo run --release --example bench -- <clips> [--model <ajuste|ruta>] [--device gpu|cpu]
//!     [--threads <n>] [--check] [--unload-check]
//!
//! La carga se mide en procesos aparte (este mismo ejemplo con `--load-only`): en frío, la de un
//! primer proceso; en caliente, la de un segundo cuando el primero ya terminó (el driver guarda sus
//! shaders compilados al cerrar el proceso, y un exe recién compilado no tiene caché). "Carga" es
//! cargar el modelo y transcribir 1 s de silencio, como hace la app antes de estar lista.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use anotalo_core::engine::{Asr, AsrFactory, Transcriber, WhisperAsr};
use anotalo_core::models::{
    CATALOG, HttpFetch, Placement, choose, cpu_threads, ensure_model, model_for, models_dir,
};
use anotalo_whisper::{LoadOptions, Model, Params, devices};
use unicode_normalization::UnicodeNormalization;

const USAGE: &str = "Uso: bench <clips> [--model <ajuste|ruta>] [--device gpu|cpu] [--threads <n>] [--check] [--unload-check]";

struct Args {
    clips: PathBuf,
    model: Option<String>,
    gpu: bool,
    threads: i32,
    check: bool,
    unload_check: bool,
    load_only: bool,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        clips: PathBuf::new(),
        model: None,
        gpu: true,
        threads: cpu_threads(),
        check: false,
        unload_check: false,
        load_only: false,
    };
    let mut clips = None;
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("Falta el valor de {arg}. {USAGE}"))
        };
        match arg.as_str() {
            "--model" => a.model = Some(value()?),
            "--device" => {
                a.gpu = match value()?.as_str() {
                    "gpu" => true,
                    "cpu" => false,
                    other => return Err(format!("--device gpu|cpu, no {other}")),
                }
            }
            "--threads" => {
                a.threads = value()?
                    .parse()
                    .map_err(|_| "--threads necesita un número".to_string())?
            }
            "--check" => a.check = true,
            "--unload-check" => a.unload_check = true,
            "--load-only" => a.load_only = true,
            s if !s.starts_with("--") && clips.is_none() => clips = Some(PathBuf::from(s)),
            other => return Err(format!("Opción desconocida: {other}. {USAGE}")),
        }
    }
    if !a.load_only {
        a.clips = clips.ok_or(USAGE)?;
    }
    Ok(a)
}

/// Ruta del modelo: una ruta que existe, o un ajuste del catálogo (se descarga si falta).
fn model_path(a: &Args) -> Result<(PathBuf, String), String> {
    if let Some(m) = &a.model
        && Path::new(m).exists()
    {
        let name = Path::new(m)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        return Ok((PathBuf::from(m), name));
    }
    let setting = a.model.as_deref().unwrap_or("auto");
    if setting != "auto" && !CATALOG.iter().any(|m| m.setting == setting) {
        return Err(format!("Modelo desconocido: {setting}"));
    }
    let file = model_for(setting, a.gpu);
    let path = ensure_model(file, &models_dir(), &HttpFetch::default(), &mut |p| {
        if p < 1.0 {
            eprint!("\rDescargando {}… {:>3.0} %", file.file, p * 100.0);
        }
    })?;
    Ok((path, file.file.to_string()))
}

fn options(gpu: bool) -> Result<LoadOptions, String> {
    if !gpu {
        return Ok(LoadOptions {
            use_gpu: false,
            gpu_device: 0,
        });
    }
    match choose("gpu", &devices(), None) {
        Placement::Gpu { device } | Placement::IntegratedGpu { device } => Ok(LoadOptions {
            use_gpu: true,
            gpu_device: device,
        }),
        Placement::Cpu => Err("No hay GPU con Vulkan".into()),
    }
}

/// Carga el modelo y transcribe 1 s de silencio: lo que tarda la app en estar lista.
fn load(path: &Path, opts: &LoadOptions, params: &Params) -> Result<(Model, f64), String> {
    let t = Instant::now();
    let mut m = Model::load(path, opts).map_err(|e| e.to_string())?;
    let loaded = t.elapsed().as_secs_f64();
    m.transcribe(&vec![0.0; 16_000], params)
        .map_err(|e| e.to_string())?;
    let total = t.elapsed().as_secs_f64();
    eprintln!(
        "CARGA modelo {loaded:.2} s + calentar {:.2} s",
        total - loaded
    );
    Ok((m, total))
}

fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    r.samples::<i16>()
        .map(|s| s.map(|v| f32::from(v) / 32768.0))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Normalización del score.py del spike: NFC, minúsculas, todo lo que no sea letra, número, espacio
/// o acento pasa a espacio.
fn norm(s: &str) -> Vec<String> {
    let s: String = s.to_lowercase().nfc().collect();
    let re = regex::Regex::new(r"[^\w\sáéíóúüñ]").unwrap();
    re.replace_all(&s, " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn edits(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            cur.push(
                (prev[j + 1] + 1)
                    .min(cur[j] + 1)
                    .min(prev[j] + usize::from(x != y)),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

/// Como el spike: el elemento int(0,95·(n−1)) de la lista ordenada.
fn p95(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[(0.95 * (s.len() - 1) as f64) as usize]
}

struct Row {
    cold_load: f64,
    warm_load: f64,
    total: f64,
    audio: f64,
    p50: f64,
    p95: f64,
    short_p50: f64,
    wer: f64,
}

fn load_in_new_process(a: &Args) -> Result<f64, String> {
    let mut cmd = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.arg("--load-only")
        .args(["--device", if a.gpu { "gpu" } else { "cpu" }])
        .args(["--threads", &a.threads.to_string()]);
    if let Some(m) = &a.model {
        cmd.args(["--model", m]);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("LOAD ")?.trim().parse().ok())
        .ok_or_else(|| {
            format!(
                "la carga en un segundo proceso falló: {}",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("")
            )
        })
}

fn bench(a: &Args) -> Result<Row, String> {
    let refs: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(a.clips.join("refs.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let clips = refs.as_array().ok_or("refs.json no es una lista")?;
    let (path, _) = model_path(a)?;
    let opts = options(a.gpu)?;
    let params = Params {
        language: "es".into(),
        threads: a.threads,
    };
    let cold_load = load_in_new_process(a)?;
    let warm_load = load_in_new_process(a)?;
    let (mut model, _) = load(&path, &opts, &params)?;

    let (mut times, mut errors, mut words, mut audio) = (Vec::new(), 0usize, 0usize, 0.0);
    let mut first20 = Vec::new();
    for (i, c) in clips.iter().enumerate() {
        let file = c["file"].as_str().ok_or("clip sin file")?;
        let pcm = read_wav(&a.clips.join(file))?;
        let t = Instant::now();
        let segs = model.transcribe(&pcm, &params).map_err(|e| e.to_string())?;
        times.push(t.elapsed().as_secs_f64());
        let text: String = segs.iter().map(|s| s.text.as_str()).collect();
        let reference = norm(c["text"].as_str().ok_or("clip sin text")?);
        errors += edits(&reference, &norm(&text));
        words += reference.len();
        audio += pcm.len() as f64 / 16_000.0;
        if i < 20 {
            first20.push(pcm);
        }
        eprint!("\r{}/{}", i + 1, clips.len());
    }
    eprintln!();
    let mut short = Vec::new();
    for pcm in &first20 {
        let t = Instant::now();
        model
            .transcribe(&pcm[..pcm.len().min(64_000)], &params)
            .map_err(|e| e.to_string())?;
        short.push(t.elapsed().as_secs_f64());
    }
    Ok(Row {
        cold_load,
        warm_load,
        total: times.iter().sum(),
        audio,
        p50: median(&times),
        p95: p95(&times),
        short_p50: median(&short),
        wer: 100.0 * errors as f64 / words as f64,
    })
}

fn check(a: &Args, r: &Row) -> bool {
    let bar: serde_json::Value = serde_json::from_str(include_str!("../../../bench/bar.json"))
        .expect("bench/bar.json válido");
    let key = if a.gpu { "gpu" } else { "cpu" };
    let max = &bar[key]["max"];
    let limit = |k: &str| max[k].as_f64().expect("límite en bar.json");
    let rows = [
        ("Total", r.total, limit("total_s")),
        ("p50 por clip", r.p50, limit("p50_s")),
        ("p95 por clip", r.p95, limit("p95_s")),
        ("WER %", r.wer, limit("wer_pct")),
        ("Carga en caliente", r.warm_load, limit("warm_load_s")),
    ];
    println!("\nMarca ({key}):");
    let mut ok = true;
    for (name, got, max) in rows {
        let pass = got <= max;
        ok &= pass;
        println!(
            "  {:<18} {got:>8.3} ≤ {max:<8} {}",
            name,
            if pass { "OK" } else { "FALLA" }
        );
    }
    ok
}

fn working_set_mb() -> f64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: estructura del tamaño indicado; el pseudo-handle del proceso actual no se cierra.
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) }.expect("memoria");
    c.WorkingSetSize as f64 / 1_048_576.0
}

fn gpu_used_mb() -> Option<f64> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Memoria antes de cargar, cargado y 5 s después de soltar el Transcriber (spec §5.2).
fn unload_check(a: &Args) -> Result<bool, String> {
    let (path, _) = model_path(a)?;
    let opts = options(a.gpu)?;
    let gpu_device = opts.gpu_device;
    let threads = a.threads;
    let _ = devices(); // Vulkan ya iniciado, como en la app
    std::thread::sleep(Duration::from_secs(1));
    let mem = || (working_set_mb(), gpu_used_mb());
    let before = mem();
    let use_gpu = opts.use_gpu;
    let factory = || -> AsrFactory {
        let path = path.clone();
        Box::new(move |_| {
            let opts = LoadOptions {
                use_gpu,
                gpu_device,
            };
            let m = Model::load(&path, &opts).map_err(|e| e.to_string())?;
            let mut asr = WhisperAsr::new(
                m,
                Params {
                    language: "es".into(),
                    threads,
                },
            );
            asr.transcribe(&vec![0.0; 16_000])?;
            Ok(Box::new(asr) as Box<dyn Asr>)
        })
    };
    let placement = if a.gpu {
        Placement::Gpu { device: gpu_device }
    } else {
        Placement::Cpu
    };
    // Tres ciclos: lo que quede retenido tras el primero no debe crecer en los siguientes (una fuga
    // crecería; el dispositivo Vulkan de ggml vive lo que el proceso y se crea una sola vez).
    let mut rows = vec![("antes".to_string(), before)];
    let mut released = Vec::new();
    for i in 1..=3 {
        let t = Transcriber::start(placement, factory(), Box::new(|_| {}))?;
        rows.push((format!("cargado {i}"), mem()));
        t.stop();
        std::thread::sleep(Duration::from_secs(5));
        let after = mem();
        rows.push((format!("liberado {i}"), after));
        released.push(after);
    }
    let fmt = |g: Option<f64>| g.map_or("—".into(), |g| format!("{g:.0} MB"));
    println!("\nMemoria          RAM (working set)   GPU (nvidia-smi)");
    for (name, (ram, gpu)) in &rows {
        println!("  {name:<12} {ram:>10.0} MB {:>18}", fmt(*gpu));
    }
    let (first, last) = (released[0], released[2]);
    let back_to_start = first.0 <= before.0 * 1.10;
    let no_growth = last.0 <= first.0 * 1.10;
    let gpu_ok = released.iter().all(|r| match (before.1, r.1) {
        (Some(b), Some(x)) => x <= b + 150.0,
        _ => true,
    });
    println!(
        "  RAM vuelve al inicio: {}   RAM no crece entre ciclos: {}   GPU liberada: {}",
        if back_to_start {
            "sí".to_string()
        } else {
            format!("no (quedan {:.0} MB)", first.0 - before.0)
        },
        if no_growth { "OK" } else { "FALLA" },
        if gpu_ok { "OK" } else { "FALLA" }
    );
    Ok(no_growth && gpu_ok)
}

fn run() -> Result<bool, String> {
    let a = parse()?;
    if a.load_only {
        let (path, _) = model_path(&a)?;
        let params = Params {
            language: "es".into(),
            threads: a.threads,
        };
        let (_m, secs) = load(&path, &options(a.gpu)?, &params)?;
        println!("LOAD {secs}");
        return Ok(true);
    }
    if a.unload_check {
        return unload_check(&a);
    }
    let (_, model_name) = model_path(&a)?;
    let r = bench(&a)?;
    let engine = format!(
        "whisper.cpp {model_name} {} {} hilos",
        if a.gpu { "Vulkan" } else { "CPU" },
        a.threads
    );
    println!(
        "\n| Motor | Carga fría / caliente | {:.1} min de audio | Por clip p50 / p95 | Frase de 4 s p50 | WER |",
        r.audio / 60.0
    );
    println!("|---|---|---|---|---|---|");
    println!(
        "| {engine} | {:.1} / {:.1} s | {:.1} s ({:.0}×) | {:.2} / {:.2} s | {:.2} s | {:.1} % |",
        r.cold_load,
        r.warm_load,
        r.total,
        r.audio / r.total,
        r.p50,
        r.p95,
        r.short_p50,
        r.wer
    );
    Ok(!a.check || check(&a, &r))
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}
