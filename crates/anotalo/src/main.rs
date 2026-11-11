//! Anotalo: transcriptor de llamadas y notas (reescritura en Rust de Transcriptor).

use std::process::ExitCode;

/// "Anotalo <versión>", lo que imprime `--version`.
fn version_line() -> String {
    format!("Anotalo {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Uso: anotalo --version");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_line_names_the_product() {
        assert_eq!(
            super::version_line(),
            format!("Anotalo {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
