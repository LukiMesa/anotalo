//! Reemplazar un archivo entero sin dejarlo a medias: se escribe al lado (`<nombre>.tmp`), se lleva
//! al disco y se cambia por el original. Si algo falla, el original queda como estaba y el
//! temporal se borra (no se quedan restos en la carpeta de grabaciones).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// `<ruta>.tmp` (p. ej. `x.md.tmp`), junto al archivo.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Cambia el contenido de `path` por `contents` de una vez.
pub fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(path);
    let written = File::create(&tmp).and_then(|mut f| {
        f.write_all(contents)?;
        // antes de cambiarlo por el original: un corte de luz no deja el archivo vacío
        f.sync_all()
    });
    let result = written.and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
