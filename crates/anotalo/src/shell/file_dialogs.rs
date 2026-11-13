//! "Guardar como" y "Elegir carpeta" de Windows (`IFileSaveDialog` / `IFileOpenDialog`), con la
//! ventana principal como dueña. Solo desde el hilo principal, que ya tiene COM de un solo hilo
//! (lo inicia `MainWindow::create`).

use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
    FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::core::{HSTRING, Interface, w};

/// La ruta elegida en un diálogo ya mostrado; None si se canceló o falló.
fn result(dialog: &IFileDialog, owner: HWND) -> Option<PathBuf> {
    // SAFETY: COM en el hilo principal; la cadena devuelta se libera con CoTaskMemFree.
    unsafe {
        if let Err(e) = dialog.Show(Some(owner)) {
            // cancelar también llega como error (ERROR_CANCELLED)
            log::debug!("Diálogo cerrado sin elegir: {e}");
            return None;
        }
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

/// "Guardar como" un Markdown, con `suggested` como nombre.
pub fn save_markdown(owner: HWND, suggested: &str) -> Option<PathBuf> {
    let filters = [
        COMDLG_FILTERSPEC {
            pszName: w!("Markdown (*.md)"),
            pszSpec: w!("*.md"),
        },
        COMDLG_FILTERSPEC {
            pszName: w!("Todos los archivos (*.*)"),
            pszSpec: w!("*.*"),
        },
    ];
    // SAFETY: COM en el hilo principal; los filtros viven durante las llamadas.
    let dialog = unsafe {
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| log::error!("No se pudo abrir \"Guardar como\": {e}"))
            .ok()?;
        let options = d.GetOptions().unwrap_or_default();
        let set = d
            .SetOptions(options | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
            .and_then(|()| d.SetFileTypes(&filters))
            .and_then(|()| d.SetDefaultExtension(w!("md")))
            .and_then(|()| d.SetFileName(&HSTRING::from(suggested)));
        if let Err(e) = set {
            log::warn!("\"Guardar como\" sin preparar del todo: {e}");
        }
        d.cast::<IFileDialog>().ok()?
    };
    result(&dialog, owner)
}

/// "Elegir carpeta".
pub fn pick_folder(owner: HWND) -> Option<PathBuf> {
    // SAFETY: COM en el hilo principal.
    let dialog = unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| log::error!("No se pudo abrir \"Elegir carpeta\": {e}"))
            .ok()?;
        let options = d.GetOptions().unwrap_or_default();
        if let Err(e) = d.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM) {
            log::warn!("\"Elegir carpeta\" sin preparar: {e}");
        }
        d.cast::<IFileDialog>().ok()?
    };
    result(&dialog, owner)
}
