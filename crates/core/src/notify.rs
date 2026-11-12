//! Avisos al usuario (en Windows, los avisos del sistema). Los textos van en español, como hoy.

use std::path::Path;

pub trait Notifier: Send + Sync {
    /// `open`: archivo que se abre al hacer clic en el aviso.
    fn notify(&self, title: &str, body: &str, open: Option<&Path>);
}
