//! "Copiar para Claude" (el texto que se copia) y "Conectar con Claude Code" (dónde está `claude`
//! y qué se le pide), con un ejecutor falso: nunca se lanza nada de verdad.
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anotalo_core::claude::{
    Connect, add_args, connect, display_command, find_claude, format_for_claude, mcp_name,
    remove_args, search_dirs,
};
use anotalo_core::paths::Profile;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn test_format_for_claude_layout() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("2026-11-11_0905_Teams.md");
    std::fs::copy(fixture("transcript_rust_title_marks.md"), &p).unwrap();
    assert_eq!(
        format_for_claude(&p, "Haz X").unwrap(),
        "Haz X\n\n<transcripcion archivo=\"2026-11-11_0905_Teams.md\">\n\
         Llamada: Teams — 2026-11-11 09:05 (00:53)\nTítulo: Reunión con Acme\nPersonas: Otros, Yo\n\n\
         [00:00:01] Otros: Hola, ¿empezamos?\n[00:00:02] (Marca)\n[00:00:03] Yo: Sí, adelante.\n</transcripcion>"
    );
}

#[test]
fn test_format_for_claude_note_client_and_long_duration() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("2026-11-12_1000_Nota.md");
    std::fs::write(
        &p,
        "# Llamada — Nota — 2026-11-12 10:00\n\n- Inicio: 10:00:00\n- Fin: 11:02:03\n\
         - Duración: 01:02:03\n- Cliente: Acme\n\n---\n\n**[00:00:01] Yo:** Comprar café\n\n",
    )
    .unwrap();
    assert_eq!(
        format_for_claude(&p, "Resume").unwrap(),
        "Resume\n\n<transcripcion archivo=\"2026-11-12_1000_Nota.md\">\n\
         Nota rápida — 2026-11-12 10:00 (1:02:03)\nCliente: Acme\nPersonas: Yo\n\n\
         [00:00:01] Yo: Comprar café\n</transcripcion>"
    );
    assert!(format_for_claude(&d.path().join("no.md"), "Resume").is_err());
}

#[test]
fn test_add_command_and_display() {
    let exe = Path::new(r"C:\Users\Ana María\AppData\Local\Programs\Anotalo\Anotalo.exe");
    assert_eq!(
        add_args("anotalo", exe),
        [
            "mcp",
            "add",
            "--scope",
            "user",
            "anotalo",
            "--",
            exe.to_str().unwrap(),
            "mcp"
        ]
    );
    assert_eq!(
        remove_args("anotalo"),
        ["mcp", "remove", "--scope", "user", "anotalo"]
    );
    assert_eq!(
        display_command("anotalo", exe),
        r#"claude mcp add --scope user anotalo -- "C:\Users\Ana María\AppData\Local\Programs\Anotalo\Anotalo.exe" mcp"#
    );
    assert_eq!(
        (mcp_name(Profile::Installed), mcp_name(Profile::Dev)),
        ("anotalo", "anotalo-dev")
    );
}

#[test]
fn test_find_claude_follows_dirs_then_pathext() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a"), d.path().join("b"));
    for (dir, f) in [(&a, "claude.cmd"), (&b, "claude.exe"), (&b, "claude.cmd")] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(f), "").unwrap();
    }
    let ext = ".COM;.EXE;.BAT;.CMD";
    assert_eq!(
        find_claude(&[a.clone(), b.clone()], ext),
        Some(a.join("claude.cmd"))
    );
    assert_eq!(
        find_claude(std::slice::from_ref(&b), ext),
        Some(b.join("claude.exe"))
    );
    assert_eq!(find_claude(&[d.path().join("nada")], ext), None);
    // Una carpeta llamada claude.exe no es el programa.
    let c = d.path().join("c");
    std::fs::create_dir_all(c.join("claude.exe")).unwrap();
    assert_eq!(find_claude(&[c], ext), None);
}

#[test]
fn test_search_dirs_order_and_known_locations() {
    let v = search_dirs(
        Some(r"C:\u1;;C:\u2".into()),
        Some(r"C:\m1;C:\U1".into()),
        None,
        Path::new(r"C:\Users\a"),
        Path::new(r"C:\Users\a\AppData\Roaming"),
    );
    assert_eq!(
        v,
        [
            r"C:\u1",
            r"C:\u2",
            r"C:\m1",
            r"C:\Users\a\.local\bin",
            r"C:\Users\a\AppData\Roaming\npm"
        ]
        .map(PathBuf::from)
    );
}

#[test]
fn test_search_dirs_expands_variables_and_quotes() {
    // El PATH del registro suele traer `%USERPROFILE%`…; aquí una variable que siempre existe.
    let root = std::env::var("SystemRoot").unwrap();
    let v = search_dirs(
        Some(r#"%SystemRoot%\bin; "C:\con espacios" ;%NO_EXISTE_ANOTALO%\y"#.into()),
        None,
        Some(format!(r"{root}\BIN\;C:\p")),
        Path::new(r"C:\Users\a"),
        Path::new(r"C:\Users\a\AppData\Roaming"),
    );
    assert_eq!(
        v,
        [
            format!(r"{root}\bin").as_str(),
            r"C:\con espacios",
            r"%NO_EXISTE_ANOTALO%\y",
            r"C:\p",
            r"C:\Users\a\.local\bin",
            r"C:\Users\a\AppData\Roaming\npm"
        ]
        .map(PathBuf::from)
    );
}

#[test]
fn test_connect_removes_then_adds() {
    let calls = RefCell::new(vec![]);
    let run = |_: &Path, a: &[String]| {
        calls.borrow_mut().push(a[1].clone());
        Ok((if a[1] == "remove" { 1 } else { 0 }, String::new()))
    };
    let r = connect(
        Some(Path::new("claude.exe")),
        "anotalo",
        Path::new(r"C:\A\Anotalo.exe"),
        &run,
    );
    assert!(matches!(r, Connect::Connected));
    assert_eq!(*calls.borrow(), ["remove", "add"]);
}

#[test]
fn test_connect_without_claude_or_with_an_error() {
    let exe = Path::new(r"C:\A\Anotalo.exe");
    match connect(None, "anotalo", exe, &|_, _| unreachable!()) {
        Connect::NotFound { command } => assert_eq!(command, display_command("anotalo", exe)),
        _ => panic!(),
    }
    match connect(Some(Path::new("claude.exe")), "anotalo", exe, &|_, _| {
        Ok((1, "boom".into()))
    }) {
        Connect::Failed { error, .. } => assert!(error.contains("boom")),
        _ => panic!(),
    }
}

#[test]
fn test_connect_error_is_trimmed_and_never_empty() {
    let exe = Path::new(r"C:\A\Anotalo.exe");
    let long = format!("  {}  \n", "e".repeat(800));
    match connect(Some(Path::new("claude.exe")), "anotalo", exe, &|_, _| {
        Ok((2, long.clone()))
    }) {
        Connect::Failed { command, error } => {
            assert_eq!(command, display_command("anotalo", exe));
            assert_eq!(error, "e".repeat(500));
        }
        _ => panic!(),
    }
    match connect(Some(Path::new("claude.exe")), "anotalo", exe, &|_, a| {
        if a[1] == "add" {
            Ok((3, " \n".into()))
        } else {
            Err("no arrancó".into())
        }
    }) {
        Connect::Failed { error, .. } => assert!(error.contains('3'), "{error}"),
        _ => panic!(),
    }
    match connect(Some(Path::new("claude.exe")), "anotalo", exe, &|_, _| {
        Err("tardó demasiado".into())
    }) {
        Connect::Failed { error, .. } => assert_eq!(error, "tardó demasiado"),
        _ => panic!(),
    }
}
