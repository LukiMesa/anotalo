//! Datos de versión del exe (SignPath exige el nombre del producto en los metadatos), icono y
//! manifiesto; la tabla de archivos de la página (`web/`) y `vcomp140.dll`, que el exe lleva
//! dentro.

use std::path::{Path, PathBuf};

/// Por usuario, sin pedir permisos (spec §6). Sin `requestedExecutionLevel`, la detección de
/// instaladores de Windows pediría ser administrador a un exe llamado `Anotalo-Setup-….exe`.
/// `supportedOS`: Windows 10 y 11.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

fn main() {
    web_assets();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed_vcomp();
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let icon = "assets/anotalo.ico";
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set_manifest(MANIFEST)
        .set("ProductName", "Anotalo")
        .set("CompanyName", "LukiMesa")
        .set("FileDescription", "Anotalo")
        .set("ProductVersion", &version)
        .set("FileVersion", &version);
    res.compile()
        .expect("no se pudo compilar el recurso de versión");
}

/// `$OUT_DIR/vcomp140.dll`: la del runtime de OpenMP que copia `anotalo-whisper` (su metadato
/// `vcomp`, por `links`). El instalador la escribe junto a `Anotalo.exe`, tal cual: firmada por
/// Microsoft.
fn embed_vcomp() {
    let from = std::env::var("DEP_ANOTALO_WHISPER_VCOMP")
        .expect("anotalo-whisper no dio la ruta de vcomp140.dll (metadato vcomp)");
    println!("cargo:rerun-if-changed={from}");
    let to = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("vcomp140.dll");
    std::fs::copy(&from, &to)
        .unwrap_or_else(|e| panic!("no se pudo copiar {from} a {}: {e}", to.display()));
}

/// `$OUT_DIR/web_assets.rs`: `ASSETS`, cada archivo de `web/` (ruta con `/`, contenido con
/// `include_bytes!`), ordenados por ruta. Se rehace si cambia algo en `web/`.
fn web_assets() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("web");
    println!("cargo:rerun-if-changed=web");
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut out = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (name, path) in &files {
        out.push_str(&format!("    ({name:?}, include_bytes!({path:?})),\n"));
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("web_assets.rs");
    std::fs::write(&dest, out).expect("no se pudo escribir la tabla de la página");
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("entrada de web/").path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let name = path
                .strip_prefix(root)
                .expect("dentro de web/")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((name, path.to_string_lossy().into_owned()));
        }
    }
}
