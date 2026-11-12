//! Señales de Windows para detectar llamadas: el registro de privacidad del micrófono y los
//! títulos de las ventanas. Las ignoradas leen este PC (no hay apps ni ventanas en CI).
//! `cargo test --release -p anotalo-winsys --test signals -- --ignored --nocapture`
use anotalo_winsys::consent::{mic_entries, nonpackaged_key};
use anotalo_winsys::windows::visible_titles;

#[test]
fn test_nonpackaged_key_format() {
    // como las claves de calltx/detector.py: "NonPackaged/" + la ruta con '#'
    assert_eq!(
        nonpackaged_key("C:#Program Files#Mozilla Firefox#firefox.exe"),
        "NonPackaged/C:#Program Files#Mozilla Firefox#firefox.exe"
    );
}

#[test]
#[ignore]
fn consent_store_lists_this_pcs_apps() {
    let entries = mic_entries().unwrap();
    for (k, (start, stop)) in &entries {
        println!("{k}: {start} {stop}");
    }
    assert!(!entries.is_empty());
    assert!(
        entries
            .keys()
            .any(|k| k.starts_with("NonPackaged/") && k.contains('#'))
    );
}

#[test]
#[ignore]
fn visible_titles_returns_only_non_empty_titles() {
    let titles = visible_titles(&["explorer.exe", "firefox.exe", "ms-teams.exe"]);
    println!("{titles:#?}");
    assert!(titles.iter().all(|t| !t.trim().is_empty()));
}
