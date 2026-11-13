//! Cuándo el exe hace de instalador o de desinstalador (el mismo archivo es la app, el instalador
//! y el desinstalador, spec §5.8).
//!
//! - `--uninstall`, con banderas `/…` o sin ellas, desinstala, se lance desde donde se lance.
//! - Instala si el exe no está en la carpeta de instalación, todos los argumentos son banderas
//!   `/…` o `--install`, y además se llama `Anotalo-Setup…`, o lleva una bandera de silencio, o
//!   `--install`. Así `target\release\anotalo.exe` sigue siendo la app.
//!
//! Silencio: `/VERYSILENT`, `/SILENT` o `/S` (el contrato de Inno que usa Python 0.6.x). Las demás
//! banderas `/…` se ignoran. Todo sin distinguir mayúsculas.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupMode {
    Install { silent: bool },
    Uninstall { silent: bool },
}

const SILENT: [&str; 3] = ["/verysilent", "/silent", "/s"];

pub fn setup_mode(args: &[String], exe: &Path, install_dir: &Path) -> Option<SetupMode> {
    let args: Vec<String> = args.iter().map(|a| a.to_lowercase()).collect();
    let flag = |a: &String| a.starts_with('/');
    let silent = args.iter().any(|a| SILENT.contains(&a.as_str()));

    if args.iter().any(|a| a == "--uninstall") {
        let rest_are_flags = args.iter().filter(|a| *a != "--uninstall").all(flag);
        return rest_are_flags.then_some(SetupMode::Uninstall { silent });
    }

    let norm = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let in_install_dir = exe.parent().is_some_and(|d| norm(d) == norm(install_dir));
    if in_install_dir || !args.iter().all(|a| flag(a) || a == "--install") {
        return None;
    }
    let named_setup = exe.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .to_lowercase()
            .starts_with("anotalo-setup")
    });
    let asked = args.iter().any(|a| a == "--install");
    (named_setup || silent || asked).then_some(SetupMode::Install { silent })
}
