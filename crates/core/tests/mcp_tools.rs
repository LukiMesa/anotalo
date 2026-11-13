//! Las herramientas MCP sobre una carpeta de grabaciones de prueba: listar, buscar, leer una
//! llamada (por páginas si es larga) y la última. Todo de solo lectura.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anotalo_core::config::read_output_dir;
use anotalo_core::mcp::server::{Tools, serve};
use anotalo_core::mcp::tools::{BUDGET_CHARS, LibraryTools};
use anotalo_core::paths::Profile;
use serde_json::{Value, json};
use tempfile::TempDir;

const ACME: &str = "# Llamada — Teams — 2026-11-10 09:00

- Inicio: 09:00:00
- Fin: 09:30:00
- Duración: 00:30:00
- Título: Reunión con Acme
- Cliente: Acme

---

**[00:00:01] Yo:** Hola.

**[00:00:05] María:** Acordamos enviar la propuesta el lunes.

**[00:05:00] Marca**

";

const MEET: &str = "# Llamada — Meet — 2026-11-11 15:00

- Inicio: 15:00:00
- Fin: 15:20:00
- Duración: 00:20:00

---

**[00:00:04] Persona 1:** La acción de mejora va primero.

";

const NOTE: &str = "# Llamada — Nota — 2026-11-12 10:00

- Inicio: 10:00:00
- Fin: 10:01:00
- Duración: 00:01:00

---

**[00:00:01] Yo:** Comprar café

";

/// Una llamada en curso: sin `- Fin:`.
const LIVE: &str = "# Llamada — Teams — 2026-11-12 11:00

- Inicio: 11:00:00
- Cliente: ACME S.A.

---

**[00:00:02] Persona 1:** Empezamos cuando queráis.

";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn folder() -> TempDir {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    std::fs::write(p.join("2026-11-10_0900_Teams.md"), ACME).unwrap();
    std::fs::write(p.join("2026-11-11_1500_Meet.md"), MEET).unwrap();
    std::fs::write(p.join("2026-11-12_1000_Nota.md"), NOTE).unwrap();
    std::fs::write(p.join("2026-11-12_1100_Teams.md"), LIVE).unwrap();
    std::fs::copy(
        fixture("transcript_python.md"),
        p.join("2026-11-09_0800_Teams.md"),
    )
    .unwrap();
    std::fs::create_dir(p.join(".papelera")).unwrap();
    std::fs::write(
        p.join(".papelera")
            .join("1700000000000__2026-11-01_0900_Teams.md"),
        ACME.replace("Acme", "Borrada"),
    )
    .unwrap();
    std::fs::write(p.join("leeme.txt"), "No es una llamada.").unwrap();
    d
}

fn call(t: &LibraryTools, name: &str, args: Value) -> Result<Vec<String>, String> {
    t.call(name, args.as_object().unwrap())
}

/// El primer bloque de texto, como JSON.
fn j(r: Result<Vec<String>, String>) -> Value {
    serde_json::from_str(&r.unwrap()[0]).unwrap()
}

fn append_line(dir: &Path, name: &str, speaker: &str, text: &str) {
    let p = dir.join(name);
    let mut s = std::fs::read_to_string(&p).unwrap();
    s += &format!("**[00:00:10] {speaker}:** {text}\n\n");
    std::fs::write(p, s).unwrap();
}

/// Una llamada con `lines` frases de `chars` caracteres.
fn write_call(dir: &Path, name: &str, lines: usize, chars: usize) {
    let mut s = String::from(
        "# Llamada — Teams — 2026-11-12 09:00\n\n- Inicio: 09:00:00\n- Fin: 11:00:00\n\
         - Duración: 02:00:00\n\n---\n\n",
    );
    for i in 0..lines {
        let t = i * 2;
        let text = format!("Frase {i:05} {}", "x".repeat(chars - 12));
        s += &format!(
            "**[{:02}:{:02}:{:02}] Persona 1:** {text}\n\n",
            t / 3600,
            t / 60 % 60,
            t % 60
        );
    }
    std::fs::write(dir.join(name), s).unwrap();
}

/// (ruta relativa, tamaño, fecha de modificación) de cada archivo y carpeta, recursivo. Los datos
/// se leen del propio archivo: los de `read_dir` vienen del índice de la carpeta, que Windows
/// actualiza con retraso.
fn snapshot(dir: &Path) -> Vec<(String, u64, SystemTime)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64, SystemTime)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let meta = std::fs::metadata(e.path()).unwrap();
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if meta.is_dir() {
                out.push((rel, 0, meta.modified().unwrap()));
                walk(root, &e.path(), out);
            } else {
                out.push((rel, meta.len(), meta.modified().unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Cada herramienta (y una que no existe) con argumentos buenos, vacíos, malos y con rutas.
fn every_call() -> Vec<(&'static str, Value)> {
    let mut v = Vec::new();
    for name in [
        "list_calls",
        "search_calls",
        "get_call",
        "get_latest_call",
        "delete_call",
    ] {
        v.push((name, json!({})));
        v.push((
            name,
            json!({"id": "2026-11-10_0900_Teams.md", "query": "acme", "client": "acme",
                   "platform": "Teams", "people": ["maria"], "from": "2026-11-01",
                   "to": "2026-11-30", "include_notes": true, "limit": 3, "from_line": 2}),
        ));
        v.push((
            name,
            json!({"id": 5, "query": [], "client": {}, "platform": 3, "people": "x",
                   "from": "ayer", "to": "2026-02-30", "include_notes": "sí",
                   "limit": "mucho", "from_line": -1}),
        ));
        for id in [
            "../x.md",
            r"..\config.json",
            "config.json",
            ".papelera",
            ".papelera/1700000000000__2026-11-01_0900_Teams.md",
            r"C:\Windows\win.ini",
            "leeme.txt",
            "2026-11-10_0900_Teams.md:otro.md",
        ] {
            v.push((name, json!({"id": id, "query": id, "client": id})));
        }
    }
    v
}

#[test]
fn test_list_calls_newest_first_without_notes() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    let v = j(call(&t, "list_calls", json!({})));
    let ids: Vec<&str> = v["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "2026-11-12_1100_Teams.md",
            "2026-11-11_1500_Meet.md",
            "2026-11-10_0900_Teams.md",
            "2026-11-09_0800_Teams.md"
        ]
    );
    assert_eq!(v["total"], 4);
    assert_eq!(v["folder"], d.path().display().to_string());
}

#[test]
fn test_list_calls_filters() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    let n = |args| j(call(&t, "list_calls", args))["total"].as_u64().unwrap();
    assert_eq!(n(json!({"client": "acme"})), 2);
    assert_eq!(n(json!({"platform": "MEET"})), 1);
    assert_eq!(n(json!({"from": "2026-11-11", "to": "2026-11-11"})), 1);
    assert_eq!(n(json!({"people": ["maria"]})), 1);
    assert_eq!(n(json!({"people": ["maria", "persona"]})), 0);
    assert_eq!(n(json!({"include_notes": true})), 5);
    let one = j(call(&t, "list_calls", json!({"limit": 1})));
    assert_eq!(
        (one["calls"].as_array().unwrap().len(), &one["total"]),
        (1, &json!(4))
    );
    assert!(call(&t, "list_calls", json!({"from": "2026-13-01"})).is_err());
    assert_eq!(
        call(&t, "list_calls", json!({"to": "ayer"})).unwrap_err(),
        "Fecha no válida: ayer (usa AAAA-MM-DD)"
    );
}

#[test]
fn test_list_calls_metadata() {
    let d = folder();
    let v = j(call(
        &LibraryTools::new(d.path().into()),
        "list_calls",
        json!({}),
    ));
    let acme = &v["calls"][2];
    assert_eq!(acme["title"], "Reunión con Acme");
    assert_eq!(acme["client"], "Acme");
    assert_eq!(acme["platform"], "Teams");
    assert_eq!(acme["kind"], "call");
    assert_eq!(acme["started_at"], "2026-11-10T09:00:00");
    assert_eq!(acme["duration_s"], 1800);
    assert_eq!(acme["people"], json!(["Yo", "María"]));
    assert_eq!(
        (&acme["marks"], &acme["finished"]),
        (&json!(1), &json!(true))
    );
    assert_eq!(v["calls"][0]["finished"], false);
    assert_eq!(v["calls"][1]["client"], Value::Null);
}

#[test]
fn test_search_is_accent_insensitive_with_snippets() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    let v = j(call(&t, "search_calls", json!({"query": "accion"})));
    assert_eq!(v["total"], 1);
    assert_eq!(v["calls"][0]["id"], "2026-11-11_1500_Meet.md");
    assert_eq!(v["calls"][0]["matches"], 1);
    assert!(
        v["calls"][0]["snippets"][0]["text"]
            .as_str()
            .unwrap()
            .contains("acción")
    );
    assert_eq!(v["calls"][0]["snippets"][0]["ts"], "00:00:04");
    let p = j(call(&t, "search_calls", json!({"query": "PROPUESTA"})));
    assert_eq!(p["calls"][0]["snippets"][0]["speaker"], "María");
    assert!(call(&t, "search_calls", json!({"query": "  "})).is_err());
    assert_eq!(
        call(&t, "search_calls", json!({})).unwrap_err(),
        "Falta el texto que buscar (query)."
    );
    // Las notas solo si se piden.
    let cafe = |args| j(call(&t, "search_calls", args))["total"].clone();
    assert_eq!(cafe(json!({"query": "cafe"})), 0);
    assert_eq!(cafe(json!({"query": "cafe", "include_notes": true})), 1);
}

#[test]
fn test_search_counts_every_match_and_keeps_five_snippets() {
    let d = folder();
    for i in 0..7 {
        append_line(
            d.path(),
            "2026-11-10_0900_Teams.md",
            "María",
            &format!("Tarea {i}"),
        );
    }
    let v = j(call(
        &LibraryTools::new(d.path().into()),
        "search_calls",
        json!({"query": "tarea"}),
    ));
    assert_eq!(v["calls"][0]["matches"], 7);
    assert_eq!(v["calls"][0]["snippets"].as_array().unwrap().len(), 5);
}

#[test]
fn test_long_lines_are_cut_around_the_match() {
    let d = folder();
    let long = format!("{}aguja{}", "a".repeat(600), "b".repeat(600));
    append_line(d.path(), "2026-11-11_1500_Meet.md", "Persona 1", &long);
    let v = j(call(
        &LibraryTools::new(d.path().into()),
        "search_calls",
        json!({"query": "aguja"}),
    ));
    let s = v["calls"][0]["snippets"][0]["text"].as_str().unwrap();
    assert!(s.contains("aguja") && s.starts_with('…') && s.ends_with('…'));
    assert!(s.chars().count() <= 242);
}

#[test]
fn test_get_call_returns_metadata_and_the_file() {
    let d = folder();
    let r = call(
        &LibraryTools::new(d.path().into()),
        "get_call",
        json!({"id": "2026-11-10_0900_Teams"}),
    )
    .unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(
        r[1],
        std::fs::read_to_string(d.path().join("2026-11-10_0900_Teams.md")).unwrap()
    );
    let m: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(
        (&m["marks"], &m["next_from_line"]),
        (&json!(["00:05:00"]), &Value::Null)
    );
    assert_eq!(m["id"], "2026-11-10_0900_Teams.md");
    assert_eq!(m["ended_at"], "2026-11-10T09:30:00");
    assert_eq!((&m["lines_total"], &m["from_line"]), (&json!(3), &json!(1)));
    assert_eq!(m["people"], json!(["Yo", "María"]));
    // La lista incluye los `.MD`: también se pueden pedir.
    std::fs::write(d.path().join("2026-11-13_0900_Zoom.MD"), ACME).unwrap();
    let t = LibraryTools::new(d.path().into());
    let all = j(call(&t, "list_calls", json!({})));
    assert!(
        all["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "2026-11-13_0900_Zoom.MD")
    );
    assert!(call(&t, "get_call", json!({"id": "2026-11-13_0900_Zoom.MD"})).is_ok());
}

#[test]
fn test_get_call_rejects_anything_outside_the_folder() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    for id in [
        "../x.md",
        r"..\x.md",
        r"C:\x.md",
        ".papelera/1700000000000__2026-11-01_0900_Teams.md",
        "nada.md",
        "leeme.txt",
        "",
    ] {
        assert!(call(&t, "get_call", json!({"id": id})).is_err(), "{id}");
    }
    assert_eq!(
        call(&t, "get_call", json!({"id": "../x.md"})).unwrap_err(),
        "No existe la llamada «../x.md»."
    );
}

#[test]
fn test_get_latest_call() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    let id = |args| j(call(&t, "get_latest_call", args))["id"].clone();
    assert_eq!(id(json!({"client": "acme"})), "2026-11-12_1100_Teams.md");
    assert_eq!(id(json!({})), "2026-11-12_1100_Teams.md");
    assert_eq!(
        call(&t, "get_latest_call", json!({"client": "globex"})).unwrap(),
        ["No hay llamadas con «globex».".to_string()]
    );
    let r = call(&t, "get_latest_call", json!({})).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&r[0]).unwrap()["finished"],
        false
    );
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        call(
            &LibraryTools::new(empty.path().into()),
            "get_latest_call",
            json!({})
        )
        .unwrap(),
        ["No hay llamadas.".to_string()]
    );
}

#[test]
fn test_long_call_is_paged() {
    let d = tempfile::tempdir().unwrap();
    write_call(d.path(), "2026-11-12_0900_Teams.md", 3000, 60); // 3000 frases de 60 caracteres
    let t = LibraryTools::new(d.path().into());
    let (mut from, mut seen) = (1u64, 0usize);
    let mut texts = Vec::new();
    loop {
        let r = call(
            &t,
            "get_call",
            json!({"id": "2026-11-12_0900_Teams.md", "from_line": from}),
        )
        .unwrap();
        assert!(r[1].chars().count() <= BUDGET_CHARS + 200);
        assert!(r[1].starts_with("# Llamada — Teams — 2026-11-12 09:00\n"));
        seen += r[1].matches("**[").count();
        texts.extend(
            r[1].lines()
                .filter(|l| l.starts_with("**["))
                .map(str::to_string),
        );
        let m = serde_json::from_str::<Value>(&r[0]).unwrap();
        assert_eq!(
            (&m["lines_total"], &m["from_line"]),
            (&json!(3000), &json!(from))
        );
        match m["next_from_line"].as_u64() {
            Some(n) => from = n,
            None => break,
        }
    }
    assert_eq!(seen, 3000);
    // Sin huecos ni repeticiones: las frases en orden.
    for (i, l) in texts.iter().enumerate() {
        assert!(l.contains(&format!("Frase {i:05} ")), "{i}: {l}");
    }
    assert!(
        call(
            &t,
            "get_call",
            json!({"id": "2026-11-12_0900_Teams.md", "from_line": 3001})
        )
        .is_err()
    );
}

#[test]
fn test_reads_python_recordings() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    let v = j(call(&t, "list_calls", json!({})));
    let py = &v["calls"][3];
    assert_eq!(py["id"], "2026-11-09_0800_Teams.md");
    assert_eq!(py["people"], json!(["Otros", "Yo"]));
    assert_eq!(py["finished"], true);
    let s = j(call(&t, "search_calls", json!({"query": "perfecto"})));
    assert_eq!(s["calls"][0]["snippets"][0]["speaker"], "Yo");
}

#[test]
fn test_tool_list_and_schemas() {
    let d = folder();
    let t = LibraryTools::new(d.path().into());
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_latest_call","arguments":{"client":"acme"}}}"#,
    ]
    .join("\n");
    let mut out = Vec::new();
    serve(input.as_bytes(), &mut out, &t).unwrap();
    let r: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tools = r[0]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["list_calls", "search_calls", "get_call", "get_latest_call"]
    );
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        assert!(tool["inputSchema"]["properties"].is_object());
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    assert_eq!(tools[1]["inputSchema"]["required"], json!(["query"]));
    assert_eq!(tools[2]["inputSchema"]["required"], json!(["id"]));
    assert_eq!(
        tools[0]["inputSchema"]["properties"]["people"]["items"]["type"],
        "string"
    );
    assert_eq!(r[1]["result"]["isError"], false);
    assert_eq!(r[1]["result"]["content"].as_array().unwrap().len(), 2);
}

#[test]
fn test_tools_never_change_the_folder() {
    let d = folder();
    std::fs::write(d.path().join("config.json"), "{roto").unwrap();
    let before = snapshot(d.path());
    let t = LibraryTools::new(d.path().into());
    for (name, args) in every_call() {
        let _ = t.call(name, args.as_object().unwrap());
    }
    let _ = read_output_dir(&d.path().join("config.json"), Profile::Installed);
    assert_eq!(snapshot(d.path()), before);
}

#[test]
fn test_mcp_source_has_no_write_calls() {
    for (file, src) in [
        ("mod.rs", include_str!("../src/mcp/mod.rs")),
        ("server.rs", include_str!("../src/mcp/server.rs")),
        ("tools.rs", include_str!("../src/mcp/tools.rs")),
    ] {
        for bad in [
            "fs::write",
            "File::create",
            "OpenOptions",
            "remove_file",
            "remove_dir",
            "fs::rename",
            "create_dir",
            "set_len",
            "fs::copy",
            "applog",
        ] {
            assert!(!src.contains(bad), "{file} usa {bad}");
        }
    }
}
