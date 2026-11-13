//! Port de tests/test_api.py (25 casos) más título, cliente, personas, voces y diagnóstico.
mod common {
    pub mod app;
    pub mod speaker;
}

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anotalo_core::app::CALL_LABEL;
use anotalo_core::config::Settings;
use anotalo_core::hotkey::validate_hotkey;
use anotalo_core::paths::Profile;
use anotalo_core::speaker::diarizer::CallVoice;
use anotalo_core::speaker::memory::VoiceStore;
use anotalo_core::speaker::naming::{CallPrints, call_prints_path, save_call_prints};
use anotalo_core::ui_api::{UiApi, UiDeps, WindowControl};
use chrono::NaiveDate;
use common::app::{Harness, harness_with};
use common::speaker::basis;
use serde_json::{Value, json};

const CALL: &str = "# Llamada — Teams — 2026-10-23 15:04

- Inicio: 15:04:12
- Fin: 15:41:03
- Duración: 00:36:51

---

**[00:00:05] Yo:** Hola, ¿qué tal?

**[00:00:09] Otros:** Necesito cambiar el botón.

";

const NOTE: &str = "# Llamada — Nota — 2026-10-22 11:20

- Inicio: 11:20:00
- Fin: 11:21:48
- Duración: 00:01:48

---

**[00:00:01] Yo:** Apuntar factura

";

const TEAMS: &str = "2026-10-23_1504_Teams.md";
const NOTA: &str = "2026-10-22_1120_Nota.md";

#[derive(Default)]
struct FakeWindow {
    calls: Mutex<Vec<&'static str>>,
}

impl WindowControl for FakeWindow {
    fn minimize(&self) {
        self.calls.lock().unwrap().push("min");
    }
    fn toggle_maximize(&self) -> bool {
        self.calls.lock().unwrap().push("max");
        true
    }
    fn hide(&self) {
        self.calls.lock().unwrap().push("hide");
    }
}

struct Env {
    api: UiApi,
    h: Harness,
    dir: tempfile::TempDir,
    data: tempfile::TempDir,
    copied: Arc<Mutex<Vec<String>>>,
    saved_to: Arc<Mutex<Option<PathBuf>>>,
    picked: Arc<Mutex<Option<PathBuf>>>,
    live: Arc<Mutex<Option<String>>>,
    capture: Arc<Mutex<Vec<bool>>>,
    window: Arc<FakeWindow>,
}

impl Env {
    fn folder(&self) -> &Path {
        self.dir.path()
    }
    fn voices_path(&self) -> PathBuf {
        self.data.path().join("voces.json")
    }
    fn prints_dir(&self) -> PathBuf {
        self.data.path().join("llamadas")
    }
    fn captured(&self) -> Vec<bool> {
        self.capture.lock().unwrap().clone()
    }
}

fn env() -> Env {
    env_with(|_| {}, "")
}

/// La carpeta con una llamada y una nota, una app de prueba que graba en ella y hoy = 23 oct 2026.
/// El atajo se valida como en la app (lo hace su aplicador).
fn env_with(edit: impl FnOnce(&mut Settings), diagnostics: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(TEAMS), CALL).unwrap();
    std::fs::write(dir.path().join(NOTA), NOTE).unwrap();
    let mut settings = Settings {
        output_dir: dir.path().to_string_lossy().into_owned(),
        ..Settings::defaults(Profile::Dev)
    };
    edit(&mut settings);
    let h = harness_with(settings, "texto");
    h.app.set_applier(
        "hotkey",
        Box::new(|v| validate_hotkey(v.as_str().unwrap_or_default()).map(|_| ())),
    );
    let copied = Arc::new(Mutex::new(Vec::new()));
    let saved_to = Arc::new(Mutex::new(None));
    let picked = Arc::new(Mutex::new(None));
    let live = Arc::new(Mutex::new(None));
    let capture = Arc::new(Mutex::new(Vec::new()));
    let window = Arc::new(FakeWindow::default());
    let (cp, st, pk, lv, cap) = (
        copied.clone(),
        saved_to.clone(),
        picked.clone(),
        live.clone(),
        capture.clone(),
    );
    let diagnostics = diagnostics.to_string();
    let api = UiApi::new(UiDeps {
        app: h.app.clone(),
        live_id: Box::new(move || lv.lock().unwrap().clone()),
        copy: Box::new(move |t| {
            cp.lock().unwrap().push(t.to_string());
            Ok(())
        }),
        save_dialog: Box::new(move |_name| st.lock().unwrap().clone()),
        folder_dialog: Box::new(move || pk.lock().unwrap().clone()),
        today: Box::new(|| NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
        device_info: Box::new(|| "GPU (Vulkan)".into()),
        hotkey_capture: Box::new(move |on| cap.lock().unwrap().push(on)),
        window: window.clone(),
        voices_path: data.path().join("voces.json"),
        prints_dir: data.path().join("llamadas"),
        diagnostics: Box::new(move || diagnostics.clone()),
        version: "9.9.9-test".into(),
    });
    Env {
        api,
        h,
        dir,
        data,
        copied,
        saved_to,
        picked,
        live,
        capture,
        window,
    }
}

fn ids(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect()
}

fn ok() -> Value {
    json!({"ok": true})
}

#[test]
fn test_list_dicts_labels() {
    let e = env();
    let items = e.api.list_recordings("all", "", None);
    let items = items.as_array().unwrap();
    let labels: Vec<&str> = items
        .iter()
        .map(|i| i["date_label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Hoy", "Ayer"]);
    let first = &items[0];
    assert_eq!(first["id"], TEAMS);
    assert!(first["kind"] == "call" && first["app"] == "Teams");
    assert!(first["time"] == "15:04" && first["duration"] == "36:51");
    assert!(first["title"] == "Llamada" && items[1]["title"] == "Nota rápida");
    assert!(first["snippet"].as_str().unwrap().contains("botón"));
}

#[test]
fn test_list_filter_and_query() {
    let e = env();
    assert_eq!(ids(&e.api.list_recordings("note", "", None)), [NOTA]);
    assert_eq!(ids(&e.api.list_recordings("all", "boton", None)), [TEAMS]);
}

#[test]
fn test_get_recording_lines_me_flag() {
    let e = env();
    let r = e.api.get_recording(TEAMS);
    assert_eq!(r["start_end"], "15:04 – 15:41");
    let lines: Vec<(&str, bool)> = r["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["speaker"].as_str().unwrap(), l["me"].as_bool().unwrap()))
        .collect();
    assert_eq!(lines, [("Yo", true), ("Otros", false)]);
    assert_eq!(r["lines"][0]["ts"], "00:00:05");
}

#[test]
fn test_copy_uses_readable() {
    let e = env();
    assert_eq!(e.api.copy(TEAMS), ok());
    assert_eq!(
        *e.copied.lock().unwrap(),
        ["[00:00:05] Yo: Hola, ¿qué tal?\n[00:00:09] Otros: Necesito cambiar el botón."]
    );
}

#[test]
fn test_delete_undo_via_api() {
    let e = env();
    let res = e.api.delete(NOTA);
    assert_eq!(res["ok"], true);
    assert!(!e.folder().join(NOTA).exists());
    #[cfg(windows)]
    {
        // la papelera queda oculta en el Explorador
        use std::os::windows::fs::MetadataExt;
        let attrs = std::fs::metadata(e.folder().join(".papelera"))
            .unwrap()
            .file_attributes();
        assert_eq!(attrs & 0x2, 0x2);
    }
    assert_eq!(e.api.undo(res["token"].as_str().unwrap()), ok());
    assert!(e.folder().join(NOTA).exists());
}

#[test]
fn test_delete_live_recording_refused() {
    let e = env();
    *e.live.lock().unwrap() = Some(TEAMS.into());
    let res = e.api.delete(TEAMS);
    assert_eq!(res["ok"], false);
    assert!(res["error"].as_str().unwrap().contains("grabando"));
    assert!(e.folder().join(TEAMS).exists());
}

#[test]
fn test_set_hotkey_invalid_returns_error() {
    let e = env();
    assert_eq!(
        e.api.set_hotkey("r"),
        json!({"ok": false, "error": "El atajo necesita Ctrl, Alt o Win"})
    );
    assert_eq!(e.api.set_hotkey("<ctrl>+<alt>+n"), ok());
}

#[test]
fn test_set_setting_unknown_key_error() {
    let e = env();
    let res = e.api.set_setting("nope", json!(1));
    assert_eq!(res["ok"], false);
    assert!(res["error"].as_str().unwrap().contains("nope"));
}

#[test]
fn test_save_as_cancel() {
    let e = env();
    assert_eq!(
        e.api.save_as(TEAMS),
        json!({"ok": false, "cancelled": true})
    );
}

#[test]
fn test_save_as_writes_markdown() {
    let e = env();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("copia.md");
    *e.saved_to.lock().unwrap() = Some(dest.clone());
    assert_eq!(
        e.api.save_as(TEAMS),
        json!({"ok": true, "path": dest.to_string_lossy()})
    );
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), CALL);
}

#[test]
fn test_unknown_id_is_error_not_exception() {
    let e = env();
    assert_eq!(e.api.get_recording("../../etc.md")["ok"], false);
    assert_eq!(e.api.copy("no-existe.md")["ok"], false);
    // desde la página llegan argumentos de cualquier tipo
    assert_eq!(e.api.call("get_recording", &[json!(5)])["ok"], false);
    assert_eq!(e.api.call("copy", &[])["ok"], false);
}

#[test]
fn test_hotkey_capture_hook() {
    let e = env();
    assert_eq!(e.api.begin_hotkey_capture(), ok());
    assert_eq!(e.api.end_hotkey_capture(), ok());
    assert_eq!(e.captured(), [true, false]);
}

#[test]
fn test_log_js_never_raises() {
    let e = env();
    assert_eq!(e.api.log_js("TypeError: x is undefined"), ok());
    assert_eq!(e.api.call("log_js", &[json!({"raro": [1, 2]})]), ok());
}

#[test]
fn test_hotkey_capture_auto_resumes() {
    let e = env();
    e.api.set_capture_timeout(Duration::from_millis(200));
    e.api.begin_hotkey_capture();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(e.captured(), [true, false]);
    e.api.end_hotkey_capture(); // no duplica la reanudación
    assert_eq!(e.captured(), [true, false]);
}

#[test]
fn test_purge_trash_removes_old() {
    let e = env();
    let tok = e.api.delete(NOTA)["token"].as_str().unwrap().to_string();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(e.api.purge_trash(0.01), json!({"ok": true, "purged": 1}));
    assert_eq!(e.api.undo(&tok)["ok"], false);
}

#[test]
fn test_undo_token_validated() {
    let e = env();
    assert_eq!(e.api.undo(r"1__..\..\x.md")["ok"], false);
}

#[test]
fn test_api_window_controls_delegate() {
    let e = env();
    e.api.window_minimize();
    assert_eq!(
        e.api.window_toggle_maximize(),
        json!({"ok": true, "maximized": true})
    );
    e.api.window_close();
    assert_eq!(*e.window.calls.lock().unwrap(), ["min", "max", "hide"]);
    for gone in ["window_start_drag", "window_start_resize"] {
        assert_eq!(
            e.api.call(gone, &[]),
            json!({"ok": false, "error": format!("Orden desconocida: {gone}")})
        );
    }
}

#[test]
fn test_release_after_saved_as_int() {
    let e = env();
    assert_eq!(e.api.get_settings()["release_after_min"], 10);
    assert_eq!(e.api.set_setting("release_after_min", json!("2")), ok());
    assert_eq!(e.h.app.settings().release_after_min, 2);
    assert_eq!(e.api.set_setting("release_after_min", json!(0)), ok());
    assert_eq!(e.h.app.settings().release_after_min, 0);
}

/// Los cuatro casos parametrizados de Python, uno tras otro.
#[test]
fn test_release_after_rejects_other_values() {
    for bad in [json!("5"), json!("x"), Value::Null, json!(-1)] {
        let e = env();
        assert_eq!(
            e.api.set_setting("release_after_min", bad.clone())["ok"],
            false,
            "{bad}"
        );
        assert_eq!(e.h.app.settings().release_after_min, 10);
    }
}

#[test]
fn test_api_toggle_call_delegates() {
    let e = env();
    assert_eq!(e.api.toggle_call(), ok());
    assert_eq!(e.h.count(), 1);
    assert_eq!(e.h.session(0).label, CALL_LABEL);
}

#[test]
fn test_get_state_reports_kind() {
    let e = env();
    e.api.toggle_call();
    assert_eq!(e.api.get_state()["kind"], "manual");
    e.api.toggle_call();
    assert_eq!(e.api.get_state()["kind"], Value::Null);
}

#[test]
fn test_platform_settings_editable() {
    let e = env();
    for key in [
        "detect_zoom",
        "detect_whatsapp",
        "detect_slack",
        "detect_discord",
        "detect_browsers",
    ] {
        assert_eq!(e.api.set_setting(key, json!(true)), ok());
    }
    assert!(e.api.get_settings().get("detect_meet").is_none());
}

#[test]
fn test_manual_call_shows_as_manual() {
    let e = env();
    std::fs::write(
        e.folder().join("2026-10-23_1700_Llamada.md"),
        CALL.replace("Teams", "Llamada"),
    )
    .unwrap();
    let items = e.api.list_recordings("all", "", None);
    let rec = items
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "2026-10-23_1700_Llamada.md")
        .unwrap();
    assert!(rec["kind"] == "call" && rec["app"] == "Manual");
}

/// Sin actualizador hasta M4 (en Python delegaba en él): en cualquier versión responden
/// "Solo en la versión instalada", sin bloquear, y el estado no trae actualización.
#[test]
fn test_api_update_methods_delegate() {
    let e = env();
    let only = json!({"ok": false, "error": "Solo en la versión instalada"});
    assert_eq!(e.api.call("check_updates", &[]), only);
    assert_eq!(e.api.call("install_update", &[]), only);
    assert_eq!(e.api.get_state()["update"], Value::Null);
}

#[test]
fn test_api_update_disabled_in_dev() {
    let e = env();
    assert_eq!(
        e.api.check_updates(),
        json!({"ok": false, "error": "Solo en la versión instalada"})
    );
    assert_eq!(e.api.install_update()["ok"], false);
    let state = e.api.get_state();
    assert_eq!(state["update"], Value::Null);
    assert_eq!(state["frozen"], false);
    assert_eq!(state["version"], "9.9.9-test");
    assert!(state["state"] == "armed" && state["hotkey"] == "<ctrl>+<alt>+r");
    assert_eq!(state["device"], "GPU (Vulkan)");
}

// --- nuevas -------------------------------------------------------------------

fn copy_fixture(e: &Env, name: &str) -> String {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/library")
        .join(name);
    std::fs::copy(src, e.folder().join(name)).unwrap();
    name.to_string()
}

#[test]
fn test_get_recording_has_title_client_people_marks() {
    let e = env();
    let id = copy_fixture(&e, "2026-11-12_1000_Teams.md");
    let r = e.api.get_recording(&id);
    assert_eq!(r["ok"], true);
    assert_eq!(r["title"], "Llamada"); // el de Python: el tipo de grabación
    assert_eq!(r["call_title"], "Revisión de la web");
    assert_eq!(r["client"], "Acme");
    assert_eq!(r["people"], json!(["María", "Persona 1"]));
    assert_eq!(r["full_date"], "12 nov 2026");
    assert_eq!(r["start_end"], "10:00 – 10:01");
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 5);
    assert_eq!(
        lines[2],
        json!({"ts": "00:00:05", "speaker": "", "me": false, "text": "", "mark": true})
    );
    assert_eq!(
        lines[1],
        json!({"ts": "00:00:03", "speaker": "Yo", "me": true, "text": "Encantado.", "mark": false})
    );
    assert_eq!(lines.iter().filter(|l| l["mark"] == true).count(), 1);
    // la lista trae el título y el cliente; la llamada sin ellos, null
    let items = e.api.list_recordings("all", "", None);
    let item = &items[0];
    assert_eq!(item["id"], id.as_str());
    assert!(item["call_title"] == "Revisión de la web" && item["client"] == "Acme");
    assert!(
        item["date_label"] == "12 nov"
            && item["snippet"] == "Hola, soy María. Y yo Juan. Empezamos."
    );
    assert!(items[1]["call_title"].is_null() && items[1]["client"].is_null());
    assert!(item.get("people").is_none() && item.get("lines").is_none());
}

const PEOPLE: &str = "# Llamada — Teams — 2026-10-23 16:00

- Inicio: 16:00:00
- Fin: 16:10:00
- Duración: 00:10:00

---

**[00:00:01] Persona 1:** Hola.

**[00:00:03] Yo:** Buenas.

**[00:00:05] Persona 2:** Sigo yo.

**[00:00:07] Persona 1:** Vale.

";

fn voice(label: &str, k: usize, seconds: f64, named: bool) -> CallVoice {
    CallVoice {
        label: label.into(),
        print: basis(k),
        seconds,
        named,
    }
}

fn stored_names(e: &Env) -> Vec<String> {
    VoiceStore::load(&e.voices_path())
        .voices
        .into_iter()
        .map(|v| v.name)
        .collect()
}

#[test]
fn test_rename_person_remembers_voice_and_refuses_reserved() {
    let e = env();
    let id = "2026-10-23_1600_Teams.md";
    let md = e.folder().join(id);
    std::fs::write(&md, PEOPLE).unwrap();
    save_call_prints(
        &e.prints_dir(),
        &md,
        vec![
            voice("Persona 1", 0, 30.0, false),
            voice("Persona 2", 0, 10.0, false),
        ],
    )
    .unwrap();

    assert_eq!(
        e.api.rename_person(id, "Persona 1", "María"),
        json!({"ok": true, "lines": 2, "remembered": true})
    );
    assert_eq!(stored_names(&e), ["María"]);
    assert_eq!(
        e.api.get_recording(id)["people"],
        json!(["María", "Persona 2"])
    );
    // juntar a una persona partida en dos: la etiqueta de la app no es un nombre
    assert_eq!(
        e.api.rename_person(id, "Persona 2", "Persona 1"),
        json!({"ok": true, "lines": 1, "remembered": false})
    );
    assert_eq!(
        e.api.rename_person(id, "Persona 1", "otros"),
        json!({"ok": true, "lines": 1, "remembered": false})
    );
    assert_eq!(stored_names(&e), ["María"]);
    let prints: CallPrints =
        serde_json::from_slice(&std::fs::read(call_prints_path(&e.prints_dir(), &md)).unwrap())
            .unwrap();
    assert!(prints.voices.iter().any(|v| v.label == "María" && v.named));

    let bad = e.api.rename_person(id, "María", "Ana: X");
    assert_eq!(bad["ok"], false);
    assert_eq!(e.api.rename_person("no-existe.md", "A", "B")["ok"], false);
    *e.live.lock().unwrap() = Some(id.into());
    let live = e.api.rename_person(id, "María", "Marta");
    assert!(live["ok"] == false && live["error"].as_str().unwrap().contains("grabando"));
}

#[test]
fn test_set_client_roundtrip_and_filter() {
    let e = env();
    assert_eq!(
        e.api.set_client(TEAMS, "  Acme  "),
        json!({"ok": true, "client": "Acme"})
    );
    assert_eq!(e.api.get_recording(TEAMS)["client"], "Acme");
    assert_eq!(
        ids(&e.api.list_recordings("all", "", Some("ACME"))),
        [TEAMS]
    );
    assert_eq!(
        ids(&e
            .api
            .call("list_recordings", &[json!("all"), json!(""), json!("ácme")])),
        [TEAMS]
    );
    assert!(ids(&e.api.list_recordings("all", "", Some("Otra"))).is_empty());
    assert_eq!(ids(&e.api.list_recordings("all", "acme", None)), [TEAMS]);
    assert_eq!(
        e.api.set_client(TEAMS, ""),
        json!({"ok": true, "client": null})
    );
    assert!(e.api.get_recording(TEAMS)["client"].is_null());
    assert_eq!(
        std::fs::read_to_string(e.folder().join(TEAMS)).unwrap(),
        CALL
    );

    assert_eq!(e.api.set_client(TEAMS, "a\nb")["ok"], false);
    assert_eq!(e.api.set_client("no-existe.md", "Acme")["ok"], false);
    *e.live.lock().unwrap() = Some(TEAMS.into());
    let live = e.api.set_client(TEAMS, "Acme");
    assert!(live["ok"] == false && live["error"].as_str().unwrap().contains("grabando"));
    assert_eq!(
        std::fs::read_to_string(e.folder().join(TEAMS)).unwrap(),
        CALL
    );
}

#[test]
fn test_voices_list_rename_forget() {
    let e = env();
    let mut store = VoiceStore::load(&e.voices_path());
    store.remember("María", &basis(0), 120.0);
    store.remember("Juan", &basis(1), 30.0);
    store.save().unwrap();
    let (a, b) = (e.folder().join(TEAMS), e.folder().join(NOTA));
    save_call_prints(&e.prints_dir(), &a, vec![voice("María", 0, 120.0, true)]).unwrap();
    save_call_prints(
        &e.prints_dir(),
        &b,
        vec![
            voice("María", 0, 20.0, true),
            voice("Persona 1", 2, 5.0, false),
        ],
    )
    .unwrap();

    assert_eq!(
        e.api.list_voices(),
        json!([{"name": "Juan", "minutes": 0.5}, {"name": "María", "minutes": 2.0}])
    );
    assert_eq!(e.api.rename_voice("Juan", " Juanito "), ok());
    assert_eq!(stored_names(&e), ["María", "Juanito"]);
    assert_eq!(e.api.rename_voice("Juanito", "Persona 2")["ok"], false);
    assert_eq!(e.api.rename_voice("Juanito", "yo")["ok"], false);
    assert_eq!(e.api.rename_voice("Juanito", "a:b")["ok"], false);
    assert_eq!(e.api.rename_voice("Nadie", "Otro")["ok"], false);

    assert_eq!(e.api.forget_voice("María"), ok());
    assert_eq!(stored_names(&e), ["Juanito"]);
    assert!(!call_prints_path(&e.prints_dir(), &a).exists());
    let left: CallPrints =
        serde_json::from_slice(&std::fs::read(call_prints_path(&e.prints_dir(), &b)).unwrap())
            .unwrap();
    let labels: Vec<&str> = left.voices.iter().map(|v| v.label.as_str()).collect();
    assert_eq!(labels, ["Persona 1"]);
    assert_eq!(
        e.api.call("list_voices", &[]),
        json!([{"name": "Juanito", "minutes": 0.5}])
    );
}

#[test]
fn test_copy_diagnostics_has_version_settings_and_log_tail() {
    let tail = "## Registro\nlínea 249\nlínea 250";
    let e = env_with(
        |s| {
            s.extra.insert("clave_rara".into(), json!(1));
        },
        tail,
    );
    assert_eq!(e.api.copy_diagnostics(), ok());
    let copied = e.copied.lock().unwrap().clone();
    assert_eq!(copied.len(), 1);
    let text = &copied[0];
    assert!(
        text.contains("## Versión\n") && text.contains("9.9.9-test"),
        "{text}"
    );
    assert!(text.contains("## Ajustes\n"), "{text}");
    assert!(
        text.contains("\"hotkey_mark\": \"<ctrl>+<alt>+m\""),
        "{text}"
    );
    assert!(text.contains("\"release_after_min\": 10"), "{text}");
    assert!(!text.contains("clave_rara"), "{text}");
    assert!(text.ends_with(tail), "{text}");
}

#[test]
fn test_choose_folder_sets_output_dir() {
    let e = env();
    assert_eq!(
        e.api.choose_folder(),
        json!({"ok": false, "cancelled": true})
    );
    let other = tempfile::tempdir().unwrap();
    let p = other.path().to_string_lossy().into_owned();
    *e.picked.lock().unwrap() = Some(other.path().to_path_buf());
    assert_eq!(e.api.choose_folder(), json!({"ok": true, "path": p}));
    assert_eq!(e.h.app.settings().output_dir, p);
    assert_eq!(e.api.list_recordings("all", "", None), json!([]));
}

#[test]
fn test_window_close_ends_hotkey_capture() {
    let e = env();
    e.api.begin_hotkey_capture();
    e.api.window_close();
    assert_eq!(e.captured(), [true, false]);
    assert_eq!(*e.window.calls.lock().unwrap(), ["hide"]);
}

#[test]
fn test_call_dispatches_by_name() {
    let e = env();
    assert_eq!(
        ids(&e.api.call("list_recordings", &[json!("note")])),
        [NOTA]
    );
    assert_eq!(ids(&e.api.call("list_recordings", &[])), [TEAMS, NOTA]);
    assert_eq!(
        ids(&e
            .api
            .call("list_recordings", &[json!("all"), Value::Null, Value::Null])),
        [TEAMS, NOTA]
    );
    assert_eq!(
        e.api
            .call("set_setting", &[json!("release_after_min"), json!(30)]),
        ok()
    );
    assert_eq!(
        e.api.call("set_hotkey_mark", &[json!("<ctrl>+<alt>+k")]),
        ok()
    );
    assert_eq!(e.h.app.settings().hotkey_mark, "<ctrl>+<alt>+k");
    assert_eq!(
        e.api.call("get_settings", &[])["hotkey_mark"],
        "<ctrl>+<alt>+k"
    );
    assert_eq!(
        e.api.call("purge_trash", &[]),
        json!({"ok": true, "purged": 0})
    );
    assert_eq!(e.api.call("get_state", &[])["kind"], Value::Null);
}

#[test]
fn test_ui_api_is_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<UiApi>();
}
