//! Portapapeles, avisos de Windows e instancia única.
//! Las ignoradas tocan el portapapeles o muestran un aviso: `-- --ignored` en este PC.
use std::path::Path;

use anotalo_winsys::{clipboard, instance, toast};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

#[test]
fn test_toast_xml_escapes_and_links_the_file() {
    let xml = toast::toast_xml(
        "Error & <aviso>",
        "Revisa \"app.log\"",
        Some(Path::new(r"C:\Llamadas\a b é.md")),
    );
    assert!(
        xml.contains("<text>Error &amp; &lt;aviso&gt;</text>"),
        "{xml}"
    );
    assert!(
        xml.contains("<text>Revisa &quot;app.log&quot;</text>"),
        "{xml}"
    );
    assert!(xml.contains(r#"activationType="protocol""#), "{xml}");
    assert!(
        xml.contains(r#"launch="file:///C:/Llamadas/a%20b%20%C3%A9.md""#),
        "{xml}"
    );
}

#[test]
fn test_toast_xml_without_file_has_no_launch() {
    let xml = toast::toast_xml("Grabando llamada", "Teams — transcripción en curso", None);
    assert!(!xml.contains("launch="), "{xml}");
    assert!(xml.contains("<text>Teams — transcripción en curso</text>"));
}

#[test]
fn test_second_acquire_fails() {
    let name = format!("AnotaloTest-{}", std::process::id());
    let first = instance::acquire(&name).expect("la primera instancia lo consigue");
    assert!(instance::acquire(&name).is_none(), "la segunda no");
    drop(first);
    assert!(
        instance::acquire(&name).is_some(),
        "al cerrarse la primera, queda libre"
    );
}

#[test]
fn test_show_event_reaches_the_first_instance() {
    let name = format!("AnotaloTestShow-{}", std::process::id());
    let (tx, rx) = std::sync::mpsc::channel();
    instance::on_show(
        &name,
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    instance::signal_show(&name);
    assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok());
}

#[test]
fn test_register_aumid_writes_display_name() {
    let aumid = format!("Anotalo.Test.{}", std::process::id());
    toast::register_aumid(&aumid, "Anotalo (prueba)").unwrap();
    let key = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{aumid}"));
    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: búfer del tamaño indicado; la clave de prueba se borra después.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            w!("DisplayName"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    // SAFETY: borra solo la clave de prueba.
    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
    assert_eq!(rc, ERROR_SUCCESS);
    let n = buf.iter().position(|&c| c == 0).unwrap();
    assert_eq!(String::from_utf16_lossy(&buf[..n]), "Anotalo (prueba)");
}

/// El icono de los avisos (Centro de actividades): `IconUri` con la ruta de un PNG.
#[test]
fn test_aumid_icon_uri_points_at_the_png() {
    let aumid = format!("Anotalo.TestIcon.{}", std::process::id());
    let png = std::env::temp_dir().join("anotalo icono á.png");
    toast::register_aumid(&aumid, "Anotalo (prueba)").unwrap();
    toast::set_aumid_icon(&aumid, &png).unwrap();
    let key = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{aumid}"));
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: búfer del tamaño indicado; la clave de prueba se borra después.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            w!("IconUri"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    // SAFETY: borra solo la clave de prueba.
    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
    assert_eq!(rc, ERROR_SUCCESS);
    let n = buf.iter().position(|&c| c == 0).unwrap();
    assert_eq!(String::from_utf16_lossy(&buf[..n]), png.to_string_lossy());
}

#[test]
#[ignore]
fn clipboard_roundtrip() {
    // Deja el portapapeles como estaba (es el del usuario).
    let before = clipboard::get_text().ok();
    clipboard::set_text("Anotalo: prueba del portapapeles áé").unwrap();
    let got = clipboard::get_text();
    if let Some(text) = before {
        clipboard::set_text(&text).unwrap();
    }
    assert_eq!(got.unwrap(), "Anotalo: prueba del portapapeles áé");
}

#[test]
#[ignore]
fn toast_appears() {
    // Manual: debe verse un aviso de Anotalo; al hacer clic se abre el archivo de prueba.
    toast::register_aumid(toast::AUMID, "Anotalo").unwrap();
    let file = std::env::temp_dir().join("anotalo-aviso de prueba.md");
    std::fs::write(&file, "# Aviso de prueba de Anotalo\n").unwrap();
    toast::show(
        toast::AUMID,
        "Aviso de prueba",
        "Comprobando los avisos de Anotalo: al hacer clic se abre un archivo de prueba",
        Some(&file),
    )
    .unwrap();
}
