//! Vida del modelo: preparar con progreso, cargar en diferido tras liberarlo, liberarlo tras un
//! rato sin uso y medir la GPU integrada una vez (spec §5.2).
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anotalo_core::config::Settings;
use anotalo_core::engine::{Asr, AsrFactory};
use anotalo_core::model::{ModelConfig, ModelGate, ModelManager, ModelStatus, igpu_check_needed};
use anotalo_core::models::Placement;
use anotalo_core::notify::Notifier;
use anotalo_core::paths::Profile;
use anotalo_whisper::{Device, DeviceKind, Segment as Text};

struct Echo(Duration);

impl Asr for Echo {
    fn transcribe(&mut self, _pcm: &[f32]) -> Result<Vec<Text>, String> {
        std::thread::sleep(self.0);
        Ok(vec![Text {
            text: " hola".into(),
            avg_logprob: -0.2,
            no_speech_prob: 0.05,
        }])
    }
}

#[derive(Default)]
struct Toasts(Mutex<Vec<(String, String)>>);

impl Notifier for Toasts {
    fn notify(&self, title: &str, body: &str, _open: Option<&Path>) {
        self.0.lock().unwrap().push((title.into(), body.into()));
    }
}

struct Rig {
    model: Arc<ModelManager>,
    toasts: Arc<Toasts>,
    loads: Arc<AtomicUsize>,
    statuses: Arc<Mutex<Vec<ModelStatus>>>,
    saved_igpu: Arc<Mutex<Vec<(bool, String)>>>,
    /// El modelo pedido a la fábrica cada vez (None = el del ajuste).
    picks: Arc<Mutex<Vec<Option<&'static str>>>>,
}

/// `load`: lo que tarda cada carga; `fail`: la carga falla; `release_s`: segundos de inactividad.
fn rig(load: Duration, fail: bool, release_s: u64, igpu: Option<(i32, String)>) -> Rig {
    let toasts = Arc::new(Toasts::default());
    let loads = Arc::new(AtomicUsize::new(0));
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let saved_igpu = Arc::new(Mutex::new(Vec::new()));
    let picks = Arc::new(Mutex::new(Vec::new()));
    let (l, st, sv, pk) = (
        loads.clone(),
        statuses.clone(),
        saved_igpu.clone(),
        picks.clone(),
    );
    let cfg = ModelConfig {
        factory: Box::new(move |pick, progress| {
            pk.lock().unwrap().push(pick);
            let l = l.clone();
            Box::new(move |p: Placement| -> Result<Box<dyn Asr>, String> {
                progress(0.5);
                progress(1.0);
                std::thread::sleep(load);
                if fail {
                    return Err("sin conexión".into());
                }
                l.fetch_add(1, Ordering::SeqCst);
                // la GPU integrada transcribe en 10 ms y el procesador en 60 ms
                let per = match p {
                    Placement::IntegratedGpu { .. } => Duration::from_millis(10),
                    _ => Duration::from_millis(60),
                };
                Ok(Box::new(Echo(per)) as Box<dyn Asr>)
            }) as AsrFactory
        }),
        placement: Box::new(|| Placement::Cpu),
        release_after: Arc::new(AtomicU64::new(release_s)),
        igpu_check: Box::new(move || igpu.clone()),
        save_igpu: Box::new(move |faster, name| sv.lock().unwrap().push((faster, name))),
    };
    let model = ModelManager::new(
        cfg,
        toasts.clone(),
        Box::new(move |s| st.lock().unwrap().push(s)),
    );
    Rig {
        model,
        toasts,
        loads,
        statuses,
        saved_igpu,
        picks,
    }
}

fn wait_for(model: &ModelManager, want: fn(&ModelStatus) -> bool) -> ModelStatus {
    let t0 = Instant::now();
    loop {
        let s = model.status();
        if want(&s) || t0.elapsed() > Duration::from_secs(10) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_prepare_reports_progress_then_ready() {
    let r = rig(Duration::ZERO, false, 600, None);
    r.model.prepare();
    assert_eq!(
        wait_for(&r.model, |s| matches!(s, ModelStatus::Ready)),
        ModelStatus::Ready
    );
    let st = r.statuses.lock().unwrap();
    assert!(st.contains(&ModelStatus::Preparing {
        progress: 0.5,
        text: "Descargando el modelo…".into()
    }));
    assert!(st.iter().any(
        |s| matches!(s, ModelStatus::Preparing { text, .. } if text == "Cargando el modelo…")
    ));
    assert!(r.model.ready());
    assert!(r.model.preparing().is_none());
}

#[test]
fn test_preparing_blocks_recording() {
    let r = rig(Duration::from_millis(300), false, 600, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Preparing { .. }));
    assert!(!r.model.ready());
    assert!(r.model.preparing().is_some());
    assert!(r.model.acquire().is_err());
}

#[test]
fn test_failed_prepare_notifies_once() {
    let r = rig(Duration::ZERO, true, 600, None);
    r.model.prepare();
    let s = wait_for(&r.model, |s| matches!(s, ModelStatus::Failed(_)));
    assert!(matches!(s, ModelStatus::Failed(_)), "{s:?}");
    std::thread::sleep(Duration::from_millis(100));
    let toasts = r.toasts.0.lock().unwrap().clone();
    assert_eq!(
        toasts,
        vec![(
            "Error".to_string(),
            "No se pudo cargar el modelo de transcripción. Revisa app.log".to_string()
        )]
    );
    assert!(!r.model.ready());
}

#[test]
fn test_acquire_after_release_starts_without_waiting() {
    let r = rig(Duration::from_millis(300), false, 1, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    r.model.tick(Instant::now() + Duration::from_secs(5));
    assert_eq!(r.model.status(), ModelStatus::Released);
    assert!(r.model.ready(), "liberado también deja grabar");
    let t0 = Instant::now();
    let t = r.model.acquire().unwrap();
    assert!(
        t0.elapsed() < Duration::from_millis(50),
        "{:?}",
        t0.elapsed()
    );
    assert_eq!(r.model.status(), ModelStatus::Ready);
    assert!(t.wait_idle(Duration::from_secs(5)));
    // la recarga termina en segundo plano: 300 ms, o algo más si el equipo va cargado
    let t1 = Instant::now();
    while r.loads.load(Ordering::SeqCst) < 2 && t1.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(r.loads.load(Ordering::SeqCst), 2);
}

#[test]
fn test_releases_after_idle_only_when_unused() {
    let r = rig(Duration::ZERO, false, 600, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    let held = r.model.acquire().unwrap();
    let later = Instant::now() + Duration::from_secs(1200);
    r.model.tick(later);
    assert_eq!(r.model.status(), ModelStatus::Ready, "una grabación lo usa");
    drop(held);
    r.model.tick(later);
    assert_eq!(r.model.status(), ModelStatus::Released);
}

#[test]
fn test_touch_postpones_release() {
    let r = rig(Duration::ZERO, false, 600, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    let now = Instant::now();
    r.model.touch(now + Duration::from_secs(500));
    r.model.tick(now + Duration::from_secs(700));
    assert_eq!(r.model.status(), ModelStatus::Ready);
    r.model.tick(now + Duration::from_secs(1200));
    assert_eq!(r.model.status(), ModelStatus::Released);
}

#[test]
fn test_never_releases_when_zero() {
    let r = rig(Duration::ZERO, false, 0, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    r.model.tick(Instant::now() + Duration::from_secs(86_400));
    assert_eq!(r.model.status(), ModelStatus::Ready);
}

#[test]
fn test_reload_drops_and_prepares() {
    let r = rig(Duration::ZERO, false, 600, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    r.model.reload();
    std::thread::sleep(Duration::from_millis(50));
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    assert_eq!(r.loads.load(Ordering::SeqCst), 2);
}

#[test]
fn test_prepare_times_the_igpu_once_and_saves_it() {
    let r = rig(
        Duration::ZERO,
        false,
        600,
        Some((0, "Intel UHD 630".into())),
    );
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    assert_eq!(
        *r.saved_igpu.lock().unwrap(),
        vec![(true, "Intel UHD 630".to_string())]
    );
}

/// La medición de la GPU integrada usa siempre `small` (spec §5.2), sea cual sea el modelo
/// elegido; la carga de después, el del ajuste.
#[test]
fn test_igpu_timing_uses_the_small_model() {
    let r = rig(
        Duration::ZERO,
        false,
        600,
        Some((0, "Intel UHD 630".into())),
    );
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    assert_eq!(*r.picks.lock().unwrap(), vec![Some("small"), None]);
}

/// "Liberar el modelo tras…" cambia en vivo (la ventana de ajustes no reinicia nada).
#[test]
fn test_release_after_changes_live() {
    let r = rig(Duration::ZERO, false, 600, None);
    r.model.prepare();
    wait_for(&r.model, |s| matches!(s, ModelStatus::Ready));
    let later = Instant::now() + Duration::from_secs(120);
    r.model.tick(later);
    assert_eq!(r.model.status(), ModelStatus::Ready, "aún no van 10 min");
    r.model.set_release_after(Duration::from_secs(60));
    r.model.tick(later);
    assert_eq!(r.model.status(), ModelStatus::Released);
}

fn dev(name: &str, kind: DeviceKind) -> Device {
    Device {
        name: name.into(),
        description: name.into(),
        kind,
    }
}

#[test]
fn test_igpu_check_needed_cases() {
    let base = Settings::defaults(Profile::Dev);
    let igpu_only = [
        dev("CPU", DeviceKind::Cpu),
        dev("Intel UHD", DeviceKind::IntegratedGpu),
    ];
    let with_dgpu = [
        dev("Intel UHD", DeviceKind::IntegratedGpu),
        dev("RTX 3080", DeviceKind::Gpu),
    ];
    assert_eq!(igpu_check_needed(&base, &with_dgpu), None);
    assert_eq!(
        igpu_check_needed(&base, &igpu_only),
        Some((0, "Intel UHD".to_string()))
    );
    let measured = Settings {
        igpu_faster: Some(false),
        igpu_name: "Intel UHD".into(),
        ..base.clone()
    };
    assert_eq!(igpu_check_needed(&measured, &igpu_only), None);
    let other = Settings {
        igpu_faster: Some(false),
        igpu_name: "AMD Radeon".into(),
        ..base.clone()
    };
    assert_eq!(
        igpu_check_needed(&other, &igpu_only),
        Some((0, "Intel UHD".to_string()))
    );
    let cpu = Settings {
        device: "cpu".into(),
        ..base
    };
    assert_eq!(igpu_check_needed(&cpu, &igpu_only), None);
}
