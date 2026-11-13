//! Cuándo el exe hace de instalador o de desinstalador, y cuándo es la app o una orden.
use std::path::Path;

use anotalo_setup::cmdline::{SetupMode, setup_mode};

#[test]
fn test_setup_mode_table() {
    let inst = Path::new(r"C:\U\AppData\Local\Programs\Anotalo");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let cases: &[(&[&str], &str, Option<SetupMode>)] = &[
        (
            &[],
            r"C:\U\Downloads\Anotalo-Setup-1.0.0.exe",
            Some(SetupMode::Install { silent: false }),
        ),
        (
            &["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"],
            r"C:\U\AppData\Local\Temp\Transcriptor-update\Anotalo-Setup-1.0.0.exe",
            Some(SetupMode::Install { silent: true }),
        ),
        (
            &["/s"],
            r"C:\U\Downloads\anotalo-setup-1.0.0-alpha.2.exe",
            Some(SetupMode::Install { silent: true }),
        ),
        (&[], r"E:\repo\target\release\anotalo.exe", None),
        (
            &["--install"],
            r"E:\repo\target\release\anotalo.exe",
            Some(SetupMode::Install { silent: false }),
        ),
        (
            &["/SILENT"],
            r"E:\repo\target\release\anotalo.exe",
            Some(SetupMode::Install { silent: true }),
        ),
        (&["mcp"], r"C:\U\Downloads\Anotalo-Setup-1.0.0.exe", None),
        (
            &["--minimized"],
            r"C:\U\AppData\Local\Programs\Anotalo\Anotalo.exe",
            None,
        ),
        (
            &["/VERYSILENT"],
            r"C:\U\AppData\Local\Programs\Anotalo\Anotalo.exe",
            None,
        ),
        (
            &["--uninstall"],
            r"C:\U\AppData\Local\Programs\Anotalo\Anotalo.exe",
            Some(SetupMode::Uninstall { silent: false }),
        ),
        (
            &["--uninstall", "/S"],
            r"C:\U\AppData\Local\Programs\Anotalo\Anotalo.exe",
            Some(SetupMode::Uninstall { silent: true }),
        ),
    ];
    for (args, exe, want) in cases {
        assert_eq!(
            setup_mode(&s(args), Path::new(exe), inst),
            *want,
            "{args:?} {exe}"
        );
    }
}

#[test]
fn test_setup_mode_edges() {
    let inst = Path::new(r"C:\U\AppData\Local\Programs\Anotalo");
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let installed = Path::new(r"c:\u\appdata\local\programs\anotalo\ANOTALO.EXE");
    // la carpeta de instalación se compara sin distinguir mayúsculas
    assert_eq!(setup_mode(&s(&["/S"]), installed, inst), None);
    // --uninstall, en cualquier mayúscula y desde cualquier sitio; otras banderas se ignoran
    assert_eq!(
        setup_mode(
            &s(&["--UNINSTALL", "/VerySilent", "/NORESTART"]),
            Path::new(r"C:\otro\x.exe"),
            inst
        ),
        Some(SetupMode::Uninstall { silent: true })
    );
    // con otras órdenes, nada de instalar ni desinstalar
    for args in [
        &["--uninstall", "mcp"][..],
        &["--install", "--uninstall"],
        &["--install", "x"],
        &["--version"],
        &["/S", "--minimized"],
    ] {
        assert_eq!(
            setup_mode(
                &s(args),
                Path::new(r"C:\U\Downloads\Anotalo-Setup-1.0.0.exe"),
                inst
            ),
            None,
            "{args:?}"
        );
    }
    // una bandera que no es de silencio no basta fuera del instalador
    assert_eq!(
        setup_mode(
            &s(&["/NORESTART"]),
            Path::new(r"E:\repo\target\release\anotalo.exe"),
            inst
        ),
        None
    );
    assert_eq!(
        setup_mode(
            &s(&["/NORESTART"]),
            Path::new(r"C:\U\Downloads\Anotalo-Setup-1.0.0 (1).exe"),
            inst
        ),
        Some(SetupMode::Install { silent: false })
    );
}
