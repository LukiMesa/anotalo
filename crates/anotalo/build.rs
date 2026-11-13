//! Datos de versión del exe (SignPath exige el nombre del producto en los metadatos) e icono, y la
//! tabla de archivos de la página (`web/`), que el exe lleva dentro.

use std::path::{Path, PathBuf};

fn main() {
    web_assets();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let icon = "../../calltx/ui/assets/icon.ico"; // hasta que llegue el icono de Anotalo (M3)
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set("ProductName", "Anotalo")
        .set("CompanyName", "LukiMesa")
        .set("FileDescription", "Anotalo")
        .set("ProductVersion", &version)
        .set("FileVersion", &version);
    res.compile()
        .expect("no se pudo compilar el recurso de versión");
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
