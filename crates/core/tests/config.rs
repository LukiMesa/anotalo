//! Port de tests/test_config.py y de parte de tests/test_paths.py, más la compatibilidad con los
//! archivos de la app de Python (fixtures de scripts/fixtures/config_reference.py).
use std::path::{Path, PathBuf};

use anotalo_core::config::{Settings, load_settings, migrate_legacy_in, save_settings};
use anotalo_core::paths::{
    Profile, data_dir_in, default_output_dir_in, instance_names, legacy_data_dir_in, profile_from,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn defaults() -> Settings {
    Settings::defaults(Profile::Dev)
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

#[test]
fn test_defaults() {
    let d = tempfile::tempdir().unwrap();
    let s = load_settings(&d.path().join("missing.json"), Profile::Dev);
    assert_eq!(s.label_me, "Yo");
    assert_eq!(s.label_others, "Otros");
    assert_eq!(s.model, "auto");
    assert_eq!(s.language, "es");
}

#[test]
fn test_roundtrip() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.json");
    let s = Settings {
        auto_mode: false,
        hotkey: "<ctrl>+<alt>+t".into(),
        ..defaults()
    };
    save_settings(&s, &p).unwrap();
    assert_eq!(load_settings(&p, Profile::Dev), s);
}

#[test]
fn test_unknown_keys_ignored() {
    // En Python se ignoraban; aquí no rompen la carga y se conservan para volver a guardarlas.
    let d = tempfile::tempdir().unwrap();
    let p = write(d.path(), "c.json", r#"{"bogus": 1, "label_me": "Ana"}"#);
    let s = load_settings(&p, Profile::Dev);
    assert_eq!(s.label_me, "Ana");
    assert_eq!(s.extra.get("bogus"), Some(&serde_json::json!(1)));
}

#[test]
fn test_partial_file_keeps_defaults() {
    let d = tempfile::tempdir().unwrap();
    let p = write(d.path(), "c.json", r#"{"auto_mode": false}"#);
    let s = load_settings(&p, Profile::Dev);
    assert!(!s.auto_mode);
    assert_eq!(s.model, "auto");
}

#[test]
fn test_corrupt_file_gives_defaults() {
    let d = tempfile::tempdir().unwrap();
    let p = write(d.path(), "c.json", "{not json");
    assert_eq!(load_settings(&p, Profile::Dev), defaults());
    let p = write(d.path(), "list.json", "[1, 2]");
    assert_eq!(load_settings(&p, Profile::Dev), defaults());
}

#[test]
fn test_new_defaults() {
    let s = defaults();
    assert!(s.copy_on_stop && s.show_indicator);
    assert!(s.autostart && !s.autostart_configured);
    assert!(s.detect_teams && s.detect_browsers);
    assert!(s.detect_zoom && s.detect_whatsapp && s.detect_slack && !s.detect_discord);
}

#[test]
fn test_defaults_auto() {
    let s = defaults();
    assert_eq!((s.device.as_str(), s.model.as_str()), ("auto", "auto"));
}

#[test]
fn test_migrate_cuda_to_auto() {
    let d = tempfile::tempdir().unwrap();
    let p = write(
        d.path(),
        "c.json",
        r#"{"device": "cuda", "model": "large-v3-turbo", "compute_type": "float16"}"#,
    );
    let s = load_settings(&p, Profile::Dev);
    assert_eq!(s.device, "auto");
    assert_eq!(s.model, "large-v3-turbo");
}

#[test]
fn test_release_after_default_is_ten_minutes() {
    assert_eq!(defaults().release_after_min, 10);
}

#[test]
fn test_migrates_detect_meet_to_browsers() {
    let d = tempfile::tempdir().unwrap();
    let p = write(
        d.path(),
        "config.json",
        r#"{"detect_meet": false, "detect_teams": true}"#,
    );
    let s = load_settings(&p, Profile::Dev);
    assert!(!s.detect_browsers && s.detect_teams && !s.detect_discord);
    assert!(!s.extra.contains_key("detect_meet"));
}

#[test]
fn test_data_dir_frozen_vs_dev() {
    let appdata = Path::new(r"C:\Users\x\AppData\Roaming");
    assert_eq!(
        data_dir_in(appdata, Profile::Installed),
        appdata.join("Anotalo")
    );
    assert_eq!(
        data_dir_in(appdata, Profile::Dev),
        appdata.join("Anotalo-dev")
    );
    assert_eq!(
        legacy_data_dir_in(appdata, Profile::Installed),
        appdata.join("CallTranscriber")
    );
    assert_eq!(
        legacy_data_dir_in(appdata, Profile::Dev),
        appdata.join("CallTranscriber-dev")
    );
}

#[test]
fn test_default_output_dir_dev_suffix() {
    let home = Path::new(r"C:\Users\José");
    assert_eq!(
        default_output_dir_in(home, Profile::Installed),
        home.join("Documents").join("Llamadas")
    );
    assert_eq!(
        default_output_dir_in(home, Profile::Dev),
        home.join("Documents").join("Llamadas (dev)")
    );
}

#[test]
fn test_instance_names_differ() {
    assert_ne!(
        instance_names(Profile::Installed),
        instance_names(Profile::Dev)
    );
    assert_eq!(
        instance_names(Profile::Installed),
        ("AnotaloMutex", "AnotaloShow")
    );
}

#[test]
fn test_dead_and_unknown_keys_round_trip() {
    let d = tempfile::tempdir().unwrap();
    let p = write(
        d.path(),
        "c.json",
        r#"{"compute_type": "int8", "save_audio": true, "future_key": {"a": [1]}}"#,
    );
    let s = load_settings(&p, Profile::Dev);
    let out = d.path().join("out.json");
    save_settings(&s, &out).unwrap();
    let back: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(back["compute_type"], "int8");
    assert_eq!(back["save_audio"], true);
    assert_eq!(back["future_key"], serde_json::json!({"a": [1]}));
}

#[test]
fn test_wrong_type_falls_back_per_field() {
    let d = tempfile::tempdir().unwrap();
    let p = write(
        d.path(),
        "c.json",
        r#"{"auto_mode": "si", "language": "en"}"#,
    );
    let s = load_settings(&p, Profile::Dev);
    assert!(s.auto_mode);
    assert_eq!(s.language, "en");
}

#[test]
fn test_utf8_bom_is_accepted() {
    let d = tempfile::tempdir().unwrap();
    let p = write(d.path(), "c.json", "\u{feff}{\"label_me\": \"Ana\"}");
    assert_eq!(load_settings(&p, Profile::Dev).label_me, "Ana");
}

#[test]
fn test_first_run_copies_legacy_config() {
    let appdata = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir_in(appdata.path(), Profile::Installed);
    std::fs::create_dir_all(&legacy).unwrap();
    write(&legacy, "config.json", r#"{"label_me": "Ana"}"#);
    assert!(migrate_legacy_in(appdata.path(), Profile::Installed).unwrap());
    let ours = data_dir_in(appdata.path(), Profile::Installed).join("config.json");
    assert_eq!(load_settings(&ours, Profile::Installed).label_me, "Ana");
    assert!(
        legacy.join("config.json").exists(),
        "la carpeta antigua se conserva"
    );
}

#[test]
fn test_legacy_copy_skipped_when_anotalo_config_exists() {
    let appdata = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir_in(appdata.path(), Profile::Installed);
    let ours = data_dir_in(appdata.path(), Profile::Installed);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::create_dir_all(&ours).unwrap();
    write(&legacy, "config.json", r#"{"label_me": "Ana"}"#);
    write(&ours, "config.json", r#"{"label_me": "Eva"}"#);
    assert!(!migrate_legacy_in(appdata.path(), Profile::Installed).unwrap());
    assert_eq!(
        load_settings(&ours.join("config.json"), Profile::Installed).label_me,
        "Eva"
    );
}

#[test]
fn test_hotkey_mark_default() {
    assert_eq!(defaults().hotkey_mark, "<ctrl>+<alt>+m");
}

#[test]
fn test_profile_from_env() {
    let local = Path::new(r"C:\Users\x\AppData\Local");
    let installed_exe = local.join(r"Programs\Anotalo\Anotalo.exe");
    let dev_exe = Path::new(r"E:\src\target\release\anotalo.exe");
    assert_eq!(
        profile_from(None, &installed_exe, local),
        Profile::Installed
    );
    assert_eq!(
        profile_from(
            None,
            Path::new(r"C:\USERS\X\APPDATA\LOCAL\PROGRAMS\ANOTALO\anotalo.exe"),
            local
        ),
        Profile::Installed
    );
    assert_eq!(profile_from(None, dev_exe, local), Profile::Dev);
    assert_eq!(
        profile_from(Some("dev"), &installed_exe, local),
        Profile::Dev
    );
    assert_eq!(
        profile_from(Some("installed"), dev_exe, local),
        Profile::Installed
    );
}

#[test]
fn test_saved_config_matches_fixture() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("config.json");
    let s = Settings {
        hotkey: "<ctrl>+<alt>+t".into(),
        detect_discord: true,
        output_dir: r"C:\Users\José\Documents\Llamadas".into(),
        ..defaults()
    };
    save_settings(&s, &out).unwrap();
    let got = std::fs::read(&out).unwrap();
    let want = std::fs::read(fixture("config_anotalo.json")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&want)
    );
}

#[test]
fn test_python_saved_config_loads() {
    let s = load_settings(&fixture("config_python.json"), Profile::Installed);
    assert!(!s.auto_mode);
    assert_eq!(s.hotkey, "<ctrl>+<alt>+t");
    assert_eq!(s.output_dir, r"C:\Users\José\Documents\Llamadas");
    assert_eq!(s.mic_device.as_deref(), Some("Micrófono (Elgato Wave:3)"));
    assert!(s.detect_discord);
    assert_eq!(s.release_after_min, 30);
    assert_eq!(s.compute_type, "int8");
    assert_eq!(s.update_notified, "0.6.4");
    assert_eq!(
        s.hotkey_mark, "<ctrl>+<alt>+m",
        "las claves nuevas toman su valor por defecto"
    );
}

#[test]
fn test_detect_enabled_reads_the_platform_setting() {
    let s = Settings {
        detect_zoom: false,
        ..defaults()
    };
    assert!(s.detect_enabled("detect_teams"));
    assert!(!s.detect_enabled("detect_zoom"));
    assert!(!s.detect_enabled("detect_nothing"));
}
