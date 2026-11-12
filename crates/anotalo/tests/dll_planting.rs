//! Las DLL de Windows que no son KnownDLLs (mmdevapi, propsys, secur32, crypt32…) deben cargarse
//! solo de System32, aunque alguien deje una con el mismo nombre junto al exe (p. ej. en Descargas).

#[test]
fn ignores_system_dlls_planted_next_to_the_exe() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("anotalo.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_anotalo"), &exe).unwrap();
    for dll in [
        "mmdevapi.dll",
        "propsys.dll",
        "secur32.dll",
        "crypt32.dll",
        "bcryptprimitives.dll",
    ] {
        std::fs::write(dir.path().join(dll), b"no es una DLL").unwrap();
    }
    let out = std::process::Command::new(&exe)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?} {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Anotalo "));
}
