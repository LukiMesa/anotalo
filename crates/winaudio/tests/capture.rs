//! Elección de fuentes (port de tests/test_sources.py, corren en CI) y captura WASAPI real en este
//! PC (ignoradas: no hay audio en los runners).
//! `cargo test --release -p anotalo-winaudio --test capture -- --ignored --nocapture`
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anotalo_core::routing::{Audio, SessionInfo};
use anotalo_core::source::{AudioSource, OthersSpec};
use anotalo_winaudio::coreaudio::Snapshot;
use anotalo_winaudio::sources::{OthersChoice, decide_mic, decide_others, root_pid_in};

const TEAMS: &str = "MSTeams_8wekyb3d8bbwe";

fn sess(endpoint: &str, name: &str, key: &str) -> SessionInfo {
    SessionInfo {
        endpoint_id: endpoint.into(),
        endpoint_name: name.into(),
        pid: 7,
        active: true,
        key: Some(key.into()),
    }
}

fn names(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn render(sessions: Vec<SessionInfo>) -> impl Fn() -> Result<Snapshot, String> {
    move || {
        Ok(Snapshot {
            sessions: sessions.clone(),
            names: names(&[("spk", "Altavoces"), ("bt", "Auriculares (manos libres)")]),
            comm: Some("spk".into()),
            console: Some("spk".into()),
        })
    }
}

fn spec(kind: Audio, exe: Option<&str>, platform: Option<&str>) -> OthersSpec {
    OthersSpec {
        kind,
        exe: exe.map(Into::into),
        platform: platform.map(Into::into),
    }
}

fn broken() -> Result<Snapshot, String> {
    Err("sin Core Audio".into())
}

#[test]
fn find_root_pid() {
    let procs: Vec<(u32, String, u32)> = [
        (10, "explorer.exe", 1),
        (100, "firefox.exe", 10),
        (101, "firefox.exe", 100),
        (102, "Firefox.exe", 100),
        (103, "firefox.exe", 100),
    ]
    .into_iter()
    .map(|(p, n, pp)| (p, n.to_string(), pp))
    .collect();
    assert_eq!(root_pid_in(&procs, "firefox.exe"), Some(100));
    assert_eq!(root_pid_in(&procs, "ms-teams.exe"), None);
}

#[test]
fn teams_others_follow_its_output_device() {
    let snap = render(vec![sess("bt", "Auriculares (manos libres)", TEAMS)]);
    let got = decide_others(
        &spec(Audio::Device, None, Some("teams")),
        &|_| Some(42),
        &|_| Ok(()),
        &snap,
    );
    assert_eq!(
        got,
        OthersChoice::Endpoint {
            id: "bt".into(),
            name: "Auriculares (manos libres)".into()
        }
    );
}

#[test]
fn process_audio_when_available_else_the_apps_device() {
    let zoom = spec(Audio::Process, Some("zoom.exe"), Some("zoom"));
    let got = decide_others(&zoom, &|_| Some(42), &|_| Ok(()), &render(vec![]));
    assert_eq!(got, OthersChoice::Process(42));
    let got = decide_others(
        &zoom,
        &|_| Some(42),
        &|_| Err("activation failed".into()),
        &render(vec![]),
    );
    assert!(
        matches!(got, OthersChoice::Endpoint { ref id, .. } if id == "spk"),
        "{got:?}"
    );
    let got = decide_others(&zoom, &|_| None, &|_| Ok(()), &render(vec![]));
    assert!(
        matches!(got, OthersChoice::Endpoint { ref id, .. } if id == "spk"),
        "{got:?}"
    );
}

#[test]
fn others_fall_back_to_the_default_output_if_core_audio_fails() {
    let got = decide_others(
        &spec(Audio::Device, None, None),
        &|_| None,
        &|_| Ok(()),
        &broken,
    );
    assert_eq!(got, OthersChoice::DefaultOutput);
}

#[test]
fn mic_is_the_one_the_call_app_uses_unless_set() {
    let cap = || {
        Ok(Snapshot {
            sessions: vec![sess("bt-mic", "Auriculares (micro)", TEAMS)],
            names: names(&[
                ("bt-mic", "Auriculares (micro)"),
                ("int", "Micrófono interno"),
                ("elg", "Micrófono (Elgato Wave:3)"),
            ]),
            comm: Some("int".into()),
            console: Some("int".into()),
        })
    };
    assert_eq!(
        decide_mic(Some("teams"), None, &cap).as_deref(),
        Some("bt-mic")
    );
    // el ajuste es el nombre del micrófono (como en la app de Python) o su id
    assert_eq!(
        decide_mic(Some("teams"), Some("Elgato"), &cap).as_deref(),
        Some("elg")
    );
    assert_eq!(
        decide_mic(Some("teams"), Some("elg"), &cap).as_deref(),
        Some("elg")
    );
    // un micrófono elegido que ya no existe: el de la app
    assert_eq!(
        decide_mic(Some("teams"), Some("Yeti"), &cap).as_deref(),
        Some("bt-mic")
    );
    let none = || {
        Ok(Snapshot {
            sessions: vec![],
            names: names(&[("int", "Micrófono interno")]),
            comm: Some("int".into()),
            console: Some("int".into()),
        })
    };
    assert_eq!(decide_mic(None, None, &none).as_deref(), Some("int"));
    assert_eq!(decide_mic(Some("teams"), None, &broken), None);
}

/// Junta lo capturado.
fn collector() -> (Arc<Mutex<Vec<f32>>>, anotalo_core::source::OnAudio) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    (
        got,
        Box::new(move |_t, x: &[f32]| sink.lock().unwrap().extend_from_slice(x)),
    )
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
#[ignore]
fn endpoint_loopback_hears_a_tone() {
    use anotalo_winaudio::capture::EndpointLoopbackSource;

    let snap = anotalo_winaudio::coreaudio::render_snapshot().unwrap();
    let console = snap.console.clone().unwrap();
    let (id, name) = snap
        .names
        .iter()
        .find(|(id, n)| n.contains("Elgato") && **id != console)
        .or_else(|| snap.names.iter().find(|(id, _)| **id != console))
        .map(|(id, n)| (id.clone(), n.clone()))
        .expect("hace falta una salida que no sea la predeterminada");
    println!("Tono en: {name}");
    let mut src = EndpointLoopbackSource::new(Some(id.clone()), name);
    let (got, on_audio) = collector();
    src.start(on_audio).unwrap();
    assert!(src.is_active());
    tone::play(&id, 2.0);
    std::thread::sleep(Duration::from_millis(300));
    src.stop();
    assert!(!src.is_active());
    let x = got.lock().unwrap();
    let level = rms(&x);
    println!("{} muestras, RMS {level:.3}", x.len());
    assert!(level > 0.05, "RMS {level}");
}

#[test]
#[ignore]
fn mic_starts_on_the_default_device() {
    use anotalo_winaudio::capture::MicSource;

    let mut src = MicSource::new(None);
    let (got, on_audio) = collector();
    src.start(on_audio).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(src.is_active());
    src.stop();
    assert!(!src.is_active());
    let n = got.lock().unwrap().len();
    println!("{n} muestras del micrófono");
    assert!(n > 16_000, "{n}");
}

#[test]
#[ignore]
fn mic_falls_back_to_the_default_when_its_device_is_gone() {
    use anotalo_winaudio::capture::MicSource;

    // Auriculares Bluetooth apagados a mitad de llamada: su id ya no existe. La sesión vuelve a
    // abrir el micrófono y debe seguir grabando con el predeterminado (como la v0.6.3).
    let gone = "{0.0.1.00000000}.{00000000-0000-0000-0000-000000000000}";
    let mut src = MicSource::new(Some(gone.into()));
    let (got, on_audio) = collector();
    src.start(on_audio).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    assert!(src.is_active());
    src.stop();
    assert!(got.lock().unwrap().len() > 8_000);
}

#[test]
#[ignore]
fn process_loopback_opens_for_a_running_app() {
    use anotalo_winaudio::capture::ProcessLoopbackSource;
    use anotalo_winaudio::sources::find_root_pid;

    // un programa con sonido abierto ahora mismo (navegador, Discord, Steam…)
    let snap = anotalo_winaudio::coreaudio::render_snapshot().unwrap();
    let exe = snap
        .sessions
        .iter()
        .filter_map(|s| s.key.as_deref()?.strip_prefix("NonPackaged/#"))
        .next()
        .expect("hace falta un programa con una sesión de audio")
        .to_string();
    let pid = find_root_pid(&exe).unwrap();
    println!("Proceso: {exe} ({pid})");
    ProcessLoopbackSource::probe_pid(pid).unwrap();
    let mut src = ProcessLoopbackSource::new(pid);
    let (_got, on_audio) = collector();
    src.start(on_audio).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(src.is_active());
    src.stop();
    assert!(!src.is_active());
}

#[test]
#[ignore]
fn process_loopback_ends_when_the_program_exits() {
    use anotalo_winaudio::capture::ProcessLoopbackSource;

    // un programa que dura ~2 s
    let mut child = std::process::Command::new("ping")
        .args(["-n", "3", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut src = ProcessLoopbackSource::new(child.id());
    let (_got, on_audio) = collector();
    src.start(on_audio).unwrap();
    assert!(src.is_active());
    child.wait().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(!src.is_active(), "sigue activa tras cerrarse el programa");
    src.stop();
}

/// Reproduce un tono de 440 Hz en una salida concreta (WASAPI, modo compartido).
mod tone {
    use std::f64::consts::TAU;
    use std::time::Duration;

    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, IAudioClient,
        IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::core::PCWSTR;

    pub fn play(endpoint_id: &str, secs: f64) {
        let id: Vec<u16> = endpoint_id.encode_utf16().chain([0]).collect();
        std::thread::spawn(move || unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            {
                let en: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
                let dev = en.GetDevice(PCWSTR(id.as_ptr())).unwrap();
                let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).unwrap();
                let fmt = WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_PCM as u16,
                    nChannels: 2,
                    nSamplesPerSec: 48_000,
                    nAvgBytesPerSec: 48_000 * 4,
                    nBlockAlign: 4,
                    wBitsPerSample: 16,
                    cbSize: 0,
                };
                client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                        2_000_000,
                        0,
                        &fmt,
                        None,
                    )
                    .unwrap();
                let size = client.GetBufferSize().unwrap();
                let out: IAudioRenderClient = client.GetService().unwrap();
                client.Start().unwrap();
                let total = (secs * 48_000.0) as u64;
                let mut n = 0u64;
                while n < total {
                    let free = size - client.GetCurrentPadding().unwrap();
                    if free > 0 {
                        let buf = out.GetBuffer(free).unwrap() as *mut i16;
                        for i in 0..free as u64 {
                            let v = ((TAU * 440.0 * (n + i) as f64 / 48_000.0).sin()
                                * 0.3
                                * 32767.0) as i16;
                            *buf.add(2 * i as usize) = v;
                            *buf.add(2 * i as usize + 1) = v;
                        }
                        out.ReleaseBuffer(free, 0).unwrap();
                        n += u64::from(free);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::thread::sleep(Duration::from_millis(250));
                client.Stop().unwrap();
            }
            CoUninitialize();
        })
        .join()
        .unwrap();
    }
}
