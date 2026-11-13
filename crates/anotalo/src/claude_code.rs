//! "Conectar con Claude Code" en Windows: dónde está `claude` y cómo se ejecuta. La lógica (qué se
//! le pide y qué se responde) está en `anotalo_core::claude`.

use std::ffi::OsString;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anotalo_core::claude::{find_claude, mcp_name, search_dirs};
use anotalo_core::paths::Profile;
use anotalo_core::ui_api::ClaudeCode;
use anotalo_winsys::registry;

/// Lo que puede tardar cada orden de `claude`.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// La variable PATH del equipo (la del usuario está en `HKCU\Environment`).
const MACHINE_ENV: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Lo que se espera a la salida de `claude` una vez terminado.
const OUTPUT_WAIT: Duration = Duration::from_secs(2);

/// Dónde buscar `claude`: el PATH del usuario y el del equipo tal como están ahora en el registro
/// (una app que arrancó con Windows tiene el de entonces, sin lo que se instaló después), el del
/// proceso, `~\.local\bin` (el instalador de Claude Code) y `%APPDATA%\npm` (npm).
pub fn dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Roaming"));
    search_dirs(
        registry::get_string("Environment", "Path"),
        registry::get_string_hklm(MACHINE_ENV, "Path"),
        std::env::var("PATH").ok(),
        &home,
        &appdata,
    )
}

/// El PATH para `claude` (y para el `node` que lanza `claude.cmd`): esas carpetas. Si alguna no
/// se puede poner en un PATH, el del proceso.
pub fn fresh_path(dirs: &[PathBuf]) -> OsString {
    std::env::join_paths(dirs).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

/// Todo lo que escriba `r`, en otro hilo (así una salida larga nunca bloquea al programa).
fn collect(r: Option<impl Read + Send + 'static>) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut r) = r {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    rx
}

/// Ejecuta `exe args` sin ventana y con ese PATH: (código de salida, salida y errores), o por qué
/// no se pudo. Si pasa `timeout`, lo termina.
pub fn run(
    exe: &Path,
    args: &[String],
    path: &OsString,
    timeout: Duration,
) -> Result<(i32, String), String> {
    let mut child = Command::new(exe)
        .args(args)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("No se pudo ejecutar {}: {e}", exe.display()))?;
    let (out, err) = (collect(child.stdout.take()), collect(child.stderr.take()));
    let end = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} no respondió en {} s",
                    exe.display(),
                    timeout.as_secs_f32().ceil()
                ));
            }
            Err(e) => return Err(format!("No se pudo esperar a {}: {e}", exe.display())),
        }
    };
    let mut text = out.recv_timeout(OUTPUT_WAIT).unwrap_or_default();
    text.extend(err.recv_timeout(OUTPUT_WAIT).unwrap_or_default());
    Ok((
        status.code().unwrap_or(-1),
        String::from_utf8_lossy(&text).into_owned(),
    ))
}

/// Lo que necesita la ventana para "Conectar con Claude Code": este exe, el nombre del perfil
/// (`anotalo` o `anotalo-dev`) y `claude` buscado y ejecutado con el PATH de ahora.
pub fn deps(p: Profile) -> ClaudeCode {
    ClaudeCode {
        name: mcp_name(p).into(),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("Anotalo.exe")),
        find: Box::new(|| {
            let pathext = std::env::var("PATHEXT").unwrap_or_default();
            find_claude(&dirs(), &pathext)
        }),
        run: Box::new(|exe, args| {
            log::info!("{} {}", exe.display(), args.join(" "));
            run(exe, args, &fresh_path(&dirs()), TIMEOUT)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn system32(exe: &str) -> PathBuf {
        PathBuf::from(std::env::var("SystemRoot").unwrap())
            .join("System32")
            .join(exe)
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn process_path() -> OsString {
        std::env::var_os("PATH").unwrap_or_default()
    }

    #[test]
    fn test_run_captures_output_and_exit_code() {
        let cmd = system32("cmd.exe");
        let (code, out) = run(
            &cmd,
            &args(&["/d", "/c", "echo", "hola"]),
            &process_path(),
            TIMEOUT,
        )
        .unwrap();
        assert_eq!((code, out.trim()), (0, "hola"));
        let (code, _) = run(
            &cmd,
            &args(&["/d", "/c", "exit", "3"]),
            &process_path(),
            TIMEOUT,
        )
        .unwrap();
        assert_eq!(code, 3);
        // stderr también cuenta: ahí escribe claude sus errores
        let (code, out) = run(
            &cmd,
            &args(&["/d", "/c", "echo", "fallo", "1>&2"]),
            &process_path(),
            TIMEOUT,
        )
        .unwrap();
        assert_eq!((code, out.trim()), (0, "fallo"));
    }

    /// Una app que arrancó con Windows tiene el PATH de entonces: `claude` (y el `node` de
    /// `claude.cmd`) reciben el de ahora.
    #[test]
    fn test_run_passes_the_path_it_is_given() {
        let path = OsString::from(r"C:\uno;C:\dos con espacios");
        let (_, out) = run(
            &system32("cmd.exe"),
            &args(&["/d", "/c", "echo", "%PATH%"]),
            &path,
            TIMEOUT,
        )
        .unwrap();
        assert_eq!(out.trim(), r"C:\uno;C:\dos con espacios");
    }

    #[test]
    fn test_run_gives_up_after_the_timeout() {
        let t0 = Instant::now();
        let r = run(
            &system32("PING.EXE"),
            &args(&["-n", "30", "127.0.0.1"]),
            &process_path(),
            Duration::from_millis(300),
        );
        assert!(r.unwrap_err().contains("no respondió"));
        assert!(t0.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn test_run_reports_a_missing_program() {
        let e = run(
            Path::new(r"C:\no-existe\claude.exe"),
            &[],
            &process_path(),
            TIMEOUT,
        )
        .unwrap_err();
        assert!(e.contains("No se pudo ejecutar"), "{e}");
    }

    /// El PATH del equipo es REG_EXPAND_SZ (`%SystemRoot%\system32;…`): se lee ya expandido.
    #[test]
    fn test_machine_path_is_read_expanded() {
        let p = registry::get_string_hklm(MACHINE_ENV, "Path").expect("PATH del equipo");
        let sys = PathBuf::from(std::env::var("SystemRoot").unwrap()).join("system32");
        assert!(
            p.to_lowercase()
                .contains(&sys.to_string_lossy().to_lowercase()),
            "{p}"
        );
        let d = dirs();
        assert!(
            d.iter().any(|x| x
                .to_string_lossy()
                .eq_ignore_ascii_case(&sys.to_string_lossy())),
            "{d:?}"
        );
    }

    #[test]
    fn test_dirs_and_fresh_path_include_the_known_locations() {
        let d = dirs();
        let home = PathBuf::from(std::env::var("USERPROFILE").unwrap());
        let appdata = PathBuf::from(std::env::var("APPDATA").unwrap());
        let has = |p: &Path| {
            d.iter().any(|x| {
                x.to_string_lossy()
                    .eq_ignore_ascii_case(&p.to_string_lossy())
            })
        };
        assert!(has(&home.join(".local").join("bin")), "{d:?}");
        assert!(has(&appdata.join("npm")), "{d:?}");
        let path = fresh_path(&d).to_string_lossy().into_owned();
        assert!(
            path.to_lowercase()
                .contains(&appdata.join("npm").to_string_lossy().to_lowercase())
        );
        assert_eq!(path.split(';').count(), d.len());
    }

    #[test]
    fn test_deps_name_and_exe() {
        let c = deps(Profile::Dev);
        assert_eq!(c.name, "anotalo-dev");
        assert_eq!(c.exe, std::env::current_exe().unwrap());
        assert_eq!(deps(Profile::Installed).name, "anotalo");
    }
}
