//! El exe es un programa de ventanas (sin consola que parpadee), y aun así sus órdenes responden
//! por tuberías y en la consola de quien las lanza. `anotalo mcp` contesta por tuberías y no
//! escribe nada.
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde_json::{Value, json};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, GetConsoleScreenBufferInfo, ReadConsoleOutputCharacterW,
    STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};

const EXE: &str = env!("CARGO_BIN_EXE_anotalo");

/// Una llamada terminada.
const CALL_MD: &str = "# Llamada — Teams — 2026-11-10 09:00

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

/// (ruta relativa, tamaño, fecha de modificación) de cada archivo y carpeta, recursivo. Los datos
/// se leen del propio archivo: los de `read_dir` vienen del índice de la carpeta, que Windows
/// actualiza con retraso.
fn walk(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let meta = std::fs::metadata(&p).unwrap();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            out.push((
                rel,
                if meta.is_dir() { 0 } else { meta.len() },
                meta.modified().unwrap(),
            ));
            if meta.is_dir() {
                go(root, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn the_exe_is_a_gui_program() {
    let b = std::fs::read(EXE).unwrap();
    let pe = u32::from_le_bytes(b[0x3c..0x40].try_into().unwrap()) as usize;
    let subsystem = u16::from_le_bytes(b[pe + 24 + 68..pe + 24 + 70].try_into().unwrap());
    assert_eq!(subsystem, 2); // IMAGE_SUBSYSTEM_WINDOWS_GUI
}

#[test]
fn version_and_usage_through_pipes() {
    let out = Command::new(EXE).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        format!("Anotalo {}", env!("CARGO_PKG_VERSION"))
    );
    let bad = Command::new(EXE).arg("otra-cosa").output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("anotalo mcp"));
}

#[test]
fn mcp_answers_over_pipes() {
    let d = tempfile::tempdir().unwrap();
    let (appdata, local, out) = (
        d.path().join("appdata"),
        d.path().join("local"),
        d.path().join("out"),
    );
    std::fs::create_dir_all(appdata.join("Anotalo-dev")).unwrap();
    std::fs::create_dir_all(&local).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        appdata.join("Anotalo-dev").join("config.json"),
        json!({"output_dir": out}).to_string(),
    )
    .unwrap();
    std::fs::write(out.join("2026-11-10_0900_Teams.md"), CALL_MD).unwrap();
    let before = walk(d.path());
    let mut child = Command::new(EXE)
        .arg("mcp")
        .env("ANOTALO_PROFILE", "dev")
        .env("APPDATA", &appdata)
        .env("LOCALAPPDATA", &local)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
                "\n",
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "\n",
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                "\n",
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_calls","arguments":{}}}"#,
                "\n"
            )
            .as_bytes(),
        )
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(
        o.status.success(),
        "{:?} {}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
    let lines: Vec<Value> = String::from_utf8(o.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        lines.iter().map(|l| l["id"].clone()).collect::<Vec<_>>(),
        [json!(1), json!(2), json!(3)]
    );
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 4);
    assert!(
        lines[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("2026-11-10_0900_Teams.md")
    );
    assert_eq!(walk(d.path()), before, "mcp no escribe nada");
}

/// Con esta variable, el test hace de terminal (ver `output_reaches_the_parent_console`).
const HELPER: &str = "ANOTALO_TEST_CONSOLE_RESULT";

/// Lanzado desde una consola sin redirigir nada (como cmd.exe lanza un programa de ventanas, sin
/// pasarle sus identificadores), lo que escribe llega a esa consola. El test se relanza a sí mismo
/// con una consola propia sin ventana, que hace de terminal y luego lee lo que quedó en ella.
#[test]
fn output_reaches_the_parent_console() {
    if let Some(result) = std::env::var_os(HELPER) {
        act_as_terminal(Path::new(&result));
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let result = d.path().join("consola.txt");
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "output_reaches_the_parent_console"])
        .env(HELPER, &result)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{status}");
    let text = std::fs::read_to_string(&result).unwrap();
    let (codes, screen) = text.split_once('\n').unwrap();
    assert_eq!(codes, "0 2", "{screen}");
    assert!(
        screen.contains(&format!("Anotalo {}", env!("CARGO_PKG_VERSION"))),
        "{screen}"
    );
    assert!(screen.contains("anotalo mcp"), "{screen}"); // el uso, por stderr
}

/// La "terminal": sin entrada ni salidas (así las recibe de cmd.exe un programa de ventanas) lanza
/// `--version` y una orden que no existe, y guarda en `result` sus códigos y el texto de su
/// consola.
fn act_as_terminal(result: &Path) {
    for h in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: deja vacío un identificador estándar de este proceso de prueba.
        unsafe { SetStdHandle(h, HANDLE::default()) }.unwrap();
    }
    let code = |arg: &str| {
        // con las salidas vacías, el hijo también las recibe vacías
        let st = Command::new(EXE).arg(arg).status().unwrap();
        st.code().unwrap_or(-1)
    };
    let codes = format!("{} {}", code("--version"), code("otra-cosa"));
    let con = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .unwrap();
    let h = HANDLE(con.as_raw_handle());
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: identificador de consola válido y estructura propia.
    unsafe { GetConsoleScreenBufferInfo(h, &mut info) }.unwrap();
    let cells = info.dwSize.X as usize * (info.dwCursorPosition.Y as usize + 1);
    let mut buf = vec![0u16; cells];
    let mut read = 0u32;
    // SAFETY: búfer propio del tamaño pedido.
    unsafe { ReadConsoleOutputCharacterW(h, &mut buf, COORD { X: 0, Y: 0 }, &mut read) }.unwrap();
    let screen = String::from_utf16_lossy(&buf[..read as usize]);
    std::fs::write(result, format!("{codes}\n{screen}")).unwrap();
}

/// El manifiesto del exe (recurso `RT_MANIFEST` 1), leído sin ejecutarlo.
fn manifest_of(exe: &str) -> String {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{
        FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LoadLibraryExW, LoadResource, LockResource,
        SizeofResource,
    };
    use windows::Win32::UI::WindowsAndMessaging::RT_MANIFEST;
    use windows::core::{HSTRING, PCWSTR};
    // SAFETY: el exe se abre como datos (no se ejecuta nada); el recurso vive hasta FreeLibrary.
    unsafe {
        let m = LoadLibraryExW(&HSTRING::from(exe), None, LOAD_LIBRARY_AS_DATAFILE).unwrap();
        let text = match FindResourceW(Some(m), PCWSTR(1 as _), RT_MANIFEST) {
            r if r.is_invalid() => String::new(),
            r => {
                let data = LoadResource(Some(m), r).unwrap();
                let len = SizeofResource(Some(m), r) as usize;
                let p = LockResource(data) as *const u8;
                String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned()
            }
        };
        let _ = FreeLibrary(m);
        text
    }
}

/// Sin `asInvoker`, Windows trataría `Anotalo-Setup-x.exe` como un instalador antiguo y pediría
/// permisos de administrador (detección de instaladores de UAC). Se instala por usuario.
#[test]
fn the_exe_runs_as_invoker() {
    let m = manifest_of(EXE);
    assert!(m.contains(r#"level="asInvoker""#), "{m}");
    assert!(m.contains(r#"uiAccess="false""#), "{m}");
    // Windows 10 y 11
    assert!(m.contains("{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"), "{m}");
}
