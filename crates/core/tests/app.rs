//! Port de tests/test_app.py (37 casos) más marcas, títulos y las paradas concurrentes.
mod common {
    pub mod app;
}

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anotalo_core::app::{AppEvent, CALL_LABEL, SessionKind, State};
use anotalo_core::detector::Detected;
use anotalo_core::model::ModelGate;
use anotalo_core::routing::{Audio, PLATFORMS};
use anotalo_core::source::OthersSpec;
use common::app::{h, harness};

fn det(key: &str, label: Option<&str>) -> Detected {
    let p = PLATFORMS.iter().find(|p| p.key == key).unwrap();
    Detected {
        platform: p,
        label: label.unwrap_or(p.name).to_string(),
        exe: p.exes.first().map(|e| e.to_string()),
        title: None,
    }
}

fn teams() -> Detected {
    det("teams", None)
}

fn spec(kind: Audio, exe: Option<&str>, platform: Option<&str>) -> OthersSpec {
    OthersSpec {
        kind,
        exe: exe.map(Into::into),
        platform: platform.map(Into::into),
    }
}

#[test]
fn test_toggle_starts_and_stops() {
    let h = h();
    h.app.toggle();
    assert_eq!(h.app.state(), State::Recording);
    assert_eq!(h.session(0).label, "Nota");
    h.app.toggle();
    assert!(h.stopped(0));
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_manual_suspends_detector() {
    let h = h();
    h.app.toggle();
    assert!(h.suspended());
    h.app.toggle();
    assert!(!h.suspended());
}

#[test]
fn test_auto_stop_ignores_manual_session() {
    let h = h();
    h.app.toggle();
    h.on_stop(teams());
    assert!(!h.stopped(0));
}

#[test]
fn test_auto_start_teams_records_its_output_device() {
    // Capturar solo el proceso de Teams da silencio en algunos PCs: se graba su dispositivo de salida
    let h = h();
    h.on_start(teams());
    let s = h.session(0);
    assert_eq!(
        (s.label.as_str(), s.others),
        ("Teams", Some(spec(Audio::Device, None, Some("teams"))))
    );
    h.on_stop(teams());
    assert!(h.stopped(0));
}

#[test]
fn test_meet_uses_firefox() {
    let h = h();
    h.on_start(det("firefox", Some("Meet")));
    assert_eq!(
        h.session(0).others,
        Some(spec(Audio::Process, Some("firefox.exe"), Some("firefox")))
    );
}

#[test]
fn test_hotkey_stop_of_auto_session_dismisses() {
    let h = h();
    h.on_start(teams());
    h.app.toggle();
    assert!(h.stopped(0));
    assert_eq!(h.det.dismissed.load(Ordering::SeqCst), 1);
}

#[test]
fn test_auto_disabled_suspends_detector() {
    let h = harness(false, "texto de Yo", true);
    assert!(h.suspended());
    assert_eq!(h.app.state(), State::Idle);
    h.app.set_auto(true);
    assert!(!h.suspended());
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_quit_stops_session_and_saves() {
    let h = h();
    h.app.toggle();
    h.app.quit();
    assert!(h.stopped(0));
    assert!(!h.det.running.load(Ordering::SeqCst));
}

#[test]
fn test_model_wait_does_not_hold_lock() {
    // Mientras se pregunta si el modelo está listo, otro hilo puede leer el estado de la app.
    let h = h();
    let free = Arc::new(Mutex::new(Vec::new()));
    let (app, f) = (Arc::downgrade(&h.app), free.clone());
    *h.model.on_ready.lock().unwrap() = Some(Box::new(move || {
        let app = app.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Some(a) = app.upgrade() {
                let _ = a.state();
                let _ = a.recording_active();
            }
            let _ = tx.send(());
        });
        f.lock()
            .unwrap()
            .push(rx.recv_timeout(Duration::from_millis(500)).is_ok());
    }));
    h.on_start(teams());
    assert_eq!(*free.lock().unwrap(), vec![true]);
    assert_eq!(h.count(), 1);
}

#[test]
fn test_no_session_after_quit() {
    let h = h();
    h.app.quit();
    h.on_start(teams());
    h.app.toggle();
    assert_eq!(h.count(), 0);
}

#[test]
fn test_note_copied_on_stop() {
    let h = h();
    h.app.toggle();
    h.app.toggle();
    assert_eq!(h.copied(), vec!["texto de Yo"]);
}

#[test]
fn test_call_not_copied() {
    let h = h();
    h.on_start(teams());
    h.app.toggle();
    assert!(h.copied().is_empty());
}

#[test]
fn test_empty_note_not_copied() {
    let h = harness(true, "", true);
    h.app.toggle();
    h.app.toggle();
    assert!(h.copied().is_empty());
    assert!(h.stopped(0));
}

#[test]
fn test_copy_disabled_setting() {
    let h = harness(true, "texto de Yo", false);
    h.app.toggle();
    h.app.toggle();
    assert!(h.copied().is_empty());
}

#[test]
fn test_session_end_event() {
    let h = h();
    h.app.toggle();
    h.app.toggle();
    assert_eq!(
        h.ended(),
        vec![(Some(PathBuf::from("Nota.md")), "Nota".to_string(), true)]
    );
}

#[test]
fn test_manual_label_is_nota() {
    let h = h();
    h.app.toggle();
    let s = h.session(0);
    assert_eq!((s.label.as_str(), s.others), ("Nota", None));
}

#[test]
fn test_apply_setting_rejected_while_recording() {
    let h = h();
    h.app.toggle();
    for (key, val) in [
        ("model", "small"),
        ("output_dir", "C:/x"),
        ("language", "en"),
    ] {
        assert_eq!(
            h.app.apply_setting(key, serde_json::json!(val)),
            Err("No se puede cambiar mientras se graba".to_string())
        );
    }
    assert_eq!(h.app.settings().model, "auto");
}

#[test]
fn test_apply_hotkey_calls_rebind() {
    let h = h();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let c = calls.clone();
    h.app.set_applier(
        "hotkey",
        Box::new(move |v| {
            c.lock().unwrap().push(v.clone());
            Ok(())
        }),
    );
    assert_eq!(
        h.app
            .apply_setting("hotkey", serde_json::json!("<ctrl>+<alt>+n")),
        Ok(())
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![serde_json::json!("<ctrl>+<alt>+n")]
    );
    assert_eq!(h.app.settings().hotkey, "<ctrl>+<alt>+n");
}

#[test]
fn test_apply_hotkey_error_keeps_old() {
    let h = h();
    h.app.set_applier(
        "hotkey",
        Box::new(|_| Err("Esa combinación ya la usa otra aplicación".into())),
    );
    assert_eq!(
        h.app
            .apply_setting("hotkey", serde_json::json!("<ctrl>+<alt>+n")),
        Err("Esa combinación ya la usa otra aplicación".to_string())
    );
    assert_eq!(h.app.settings().hotkey, "<ctrl>+<alt>+r");
}

#[test]
fn test_apply_unknown_key() {
    let h = h();
    let err = h
        .app
        .apply_setting("nope", serde_json::json!(1))
        .unwrap_err();
    assert!(err.contains("desconocido"), "{err}");
}

#[test]
fn test_apply_auto_mode_and_saves() {
    let h = h();
    assert_eq!(
        h.app.apply_setting("auto_mode", serde_json::json!(false)),
        Ok(())
    );
    assert!(h.suspended());
    assert_eq!(h.app.state(), State::Idle);
    assert!(h.saved.load(Ordering::SeqCst) > 0);
}

#[test]
fn test_stopping_event_before_stop() {
    let h = h();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s, sessions) = (seen.clone(), h.sessions.clone());
    h.set_hook(Box::new(move |_app, e| {
        if let AppEvent::SessionStopping { label } = e {
            let stopped = sessions.lock().unwrap()[0].stopped.load(Ordering::SeqCst);
            s.lock().unwrap().push((label.clone(), stopped));
        }
    }));
    h.app.toggle();
    h.app.toggle();
    assert_eq!(*seen.lock().unwrap(), vec![("Nota".to_string(), false)]);
}

#[test]
fn test_after_apply_runs_with_new_value() {
    let h = h();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s, app) = (seen.clone(), Arc::downgrade(&h.app));
    h.app.set_after_apply(
        "model",
        Box::new(move |_v| {
            s.lock()
                .unwrap()
                .push(app.upgrade().unwrap().settings().model);
        }),
    );
    assert_eq!(
        h.app.apply_setting("model", serde_json::json!("small")),
        Ok(())
    );
    assert_eq!(*seen.lock().unwrap(), vec!["small"]);
}

#[test]
fn test_setting_blocked_while_stopping() {
    let h = h();
    h.app.toggle();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    h.set_hook(Box::new(move |app, e| {
        if matches!(e, AppEvent::SessionStopping { .. }) {
            s.lock()
                .unwrap()
                .push(app.apply_setting("model", serde_json::json!("small")));
        }
    }));
    h.app.toggle();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![Err("No se puede cambiar mientras se graba".to_string())]
    );
    assert!(!h.app.recording_active());
}

#[test]
fn test_toggle_while_preparing_notifies_no_session() {
    let h = h();
    *h.model.preparing.lock().unwrap() = Some((0.4, "Descargando".into()));
    h.model.not_ready.store(true, Ordering::SeqCst);
    h.app.toggle();
    assert_eq!(h.count(), 0);
    assert!(
        h.notes()
            .iter()
            .any(|n| n.0.to_lowercase().contains("preparando"))
    );
}

#[test]
fn test_call_start_while_preparing_ignored() {
    let h = h();
    *h.model.preparing.lock().unwrap() = Some((0.1, "Descargando".into()));
    h.model.not_ready.store(true, Ordering::SeqCst);
    h.on_start(teams());
    assert_eq!(h.count(), 0);
}

#[test]
fn test_state_preparing() {
    let h = h();
    *h.model.preparing.lock().unwrap() = Some((0.5, "x".into()));
    h.app.model_changed();
    assert_eq!(h.app.state(), State::Preparing);
    assert_eq!(
        h.events.lock().unwrap().last(),
        Some(&AppEvent::State(State::Preparing))
    );
    *h.model.preparing.lock().unwrap() = None;
    h.app.model_changed();
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_non_blocking_progress_keeps_recording_available() {
    // El modelo se liberó y se recarga al grabar: no hay preparación que bloquee.
    let h = h();
    assert!(h.model.preparing().is_none());
    assert_eq!(h.app.state(), State::Armed);
    h.app.toggle();
    assert_eq!(h.count(), 1);
}

#[test]
fn test_failed_model_retries_on_hotkey() {
    let h = h();
    h.model.not_ready.store(true, Ordering::SeqCst); // no hay modelo y no se está preparando
    h.app.toggle();
    assert_eq!(h.count(), 0);
    assert_eq!(h.model.retries.load(Ordering::SeqCst), 1);
    assert!(
        h.notes()
            .iter()
            .any(|n| (n.0.clone() + &n.1).to_lowercase().contains("reintent"))
    );
}

#[test]
fn test_manual_call_uses_device_audio_and_call_label() {
    let h = h();
    h.app.start_call_manual();
    let s = h.session(0);
    assert_eq!(s.label, CALL_LABEL);
    assert_eq!(CALL_LABEL, "Llamada");
    assert_eq!(s.others, Some(spec(Audio::Device, None, None)));
    assert_eq!(h.app.session_kind(), Some(SessionKind::Manual));
}

#[test]
fn test_manual_call_suspends_detector_and_hotkey_stops_it() {
    let h = harness(true, "hola", true);
    h.app.start_call_manual();
    assert!(h.suspended());
    h.app.toggle(); // el atajo de notas para la llamada
    assert!(h.stopped(0));
    assert!(h.copied().is_empty()); // las llamadas nunca se copian
    assert_eq!(h.app.session_kind(), None);
    assert!(!h.suspended());
}

#[test]
fn test_manual_call_not_started_while_recording() {
    let h = h();
    h.app.toggle(); // nota rápida en curso
    h.app.start_call_manual();
    assert_eq!(h.count(), 1);
    assert_eq!(h.app.session_kind(), Some(SessionKind::Note));
}

#[test]
fn test_toggle_call_starts_and_stops() {
    let h = h();
    h.app.toggle_call();
    assert_eq!(h.app.session_kind(), Some(SessionKind::Manual));
    h.app.toggle_call();
    assert!(h.stopped(0));
    assert_eq!(h.app.session_kind(), None);
}

#[test]
fn test_detected_platform_label_and_audio() {
    let h = h();
    h.on_start(det("whatsapp", None));
    let s = h.session(0);
    assert_eq!(
        (s.label.as_str(), s.others),
        (
            "WhatsApp",
            Some(spec(Audio::Device, None, Some("whatsapp")))
        )
    );
    assert_eq!(h.app.session_kind(), Some(SessionKind::Call));
    h.on_stop(det("whatsapp", None));
    h.on_start(det("zoom", None));
    assert_eq!(
        h.session(1).others,
        Some(spec(Audio::Process, Some("zoom.exe"), Some("zoom")))
    );
}

#[test]
fn test_recording_active_includes_saving() {
    let h = h();
    assert!(!h.app.recording_active());
    h.app.toggle();
    assert!(h.app.recording_active());
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    // la sesión ya estaba creada con retraso 0: se crea otra que tarde en cerrar
    h.app.toggle();
    h.app.toggle();
    let app = h.app.clone();
    let stopper = std::thread::spawn(move || app.toggle());
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        h.app.recording_active(),
        "cerrando la grabación (últimas frases)"
    );
    stopper.join().unwrap();
    assert!(!h.app.recording_active());
}

#[test]
fn test_stopping_manual_call_dismisses_platforms_in_use() {
    let h = h();
    h.app.start_call_manual();
    h.app.toggle_call();
    assert_eq!(h.det.dismissed_in_use.load(Ordering::SeqCst), 1);
    h.app.toggle(); // una nota rápida no descarta nada
    h.app.toggle();
    assert_eq!(h.det.dismissed_in_use.load(Ordering::SeqCst), 1);
}

#[test]
fn test_no_new_recording_while_updating() {
    // Tras lanzar el instalador, la app se cerrará en segundos: no empezar grabaciones que se perderían.
    let h = h();
    h.app.set_updating(true);
    h.app.toggle();
    h.app.start_call_manual();
    h.on_start(teams());
    assert_eq!(h.count(), 0);
    assert!(h.notes().iter().any(|n| n.0.contains("Actualizando")));
}

#[test]
fn test_mark_during_call_writes_and_emits() {
    let h = h();
    h.on_start(teams());
    assert!(h.app.mark());
    assert_eq!(h.marks.load(Ordering::SeqCst), 1);
    assert!(h.events.lock().unwrap().contains(&AppEvent::Marked {
        hms: "00:01:15".into()
    }));
    h.app.toggle();
    h.app.start_call_manual();
    assert!(h.app.mark(), "también en una llamada grabada a mano");
}

#[test]
fn test_mark_during_note_does_nothing() {
    let h = h();
    assert!(!h.app.mark(), "sin grabación");
    h.app.toggle();
    assert!(!h.app.mark(), "en una nota rápida");
    assert_eq!(h.marks.load(Ordering::SeqCst), 0);
}

#[test]
fn test_hotkey_while_saving_does_not_start_a_second_session() {
    let h = h();
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    h.app.toggle(); // nota que tarda 300 ms en cerrarse
    let app = h.app.clone();
    let first = std::thread::spawn(move || app.toggle()); // la para
    std::thread::sleep(Duration::from_millis(50));
    let app = h.app.clone();
    let second = std::thread::spawn(move || app.toggle()); // se pulsa otra vez mientras se guarda
    first.join().unwrap();
    second.join().unwrap();
    // la pulsación era para parar la que se estaba guardando: no se repite al terminar
    let ends = h.stop_ends.lock().unwrap().clone();
    let sessions = h.sessions.lock().unwrap().clone();
    assert_eq!(
        (sessions.len(), ends.len()),
        (1, 1),
        "una nota y un guardado"
    );
    assert!(sessions[0].at < ends[0]);
    assert_eq!(h.app.state(), State::Armed);
}

#[test]
fn test_record_call_while_saving_does_not_start_a_session() {
    let h = h();
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    h.app.toggle_call();
    let app = h.app.clone();
    let first = std::thread::spawn(move || app.toggle_call());
    std::thread::sleep(Duration::from_millis(50));
    let app = h.app.clone();
    let second = std::thread::spawn(move || app.toggle_call());
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(h.sessions.lock().unwrap().len(), 1);
}

#[test]
fn test_quit_during_saving_waits_for_the_file() {
    let h = h();
    *h.stop_delay.lock().unwrap() = Duration::from_millis(300);
    h.app.toggle();
    let app = h.app.clone();
    let stopper = std::thread::spawn(move || app.toggle());
    std::thread::sleep(Duration::from_millis(50));
    h.app.quit();
    assert!(
        h.stopped(0),
        "quit vuelve cuando el archivo ya está cerrado"
    );
    stopper.join().unwrap();
}

#[test]
fn test_detected_title_reaches_the_session() {
    let h = h();
    h.on_start(Detected {
        title: Some("Reunión con Acme".into()),
        ..teams()
    });
    assert_eq!(h.session(0).title.as_deref(), Some("Reunión con Acme"));
}
