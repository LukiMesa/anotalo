//! El modelo de voces se descarga y carga en segundo plano; si falla, se reintenta la próxima vez.
use std::io::{self, Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anotalo_core::models::Fetch;
use anotalo_core::speaker::model::SpeakerModel;

/// Sirve siempre los mismos bytes y cuenta las descargas.
struct CountingFetch {
    data: Vec<u8>,
    calls: AtomicUsize,
}

impl CountingFetch {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Fetch for CountingFetch {
    fn open(&self, _url: &str, _from_byte: u64) -> io::Result<Box<dyn Read>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Cursor::new(self.data.clone())))
    }
}

#[test]
fn test_failed_download_leaves_no_model_and_retries() {
    let d = tempfile::tempdir().unwrap();
    let f = Arc::new(CountingFetch {
        data: b"no es el modelo".to_vec(),
        calls: AtomicUsize::new(0),
    });
    let m = SpeakerModel::new(d.path().to_path_buf(), f.clone());
    m.ensure();
    assert!(m.wait_idle(Duration::from_secs(10)));
    assert!(m.get().is_none());
    m.ensure();
    assert!(m.wait_idle(Duration::from_secs(10)));
    assert!(m.get().is_none());
    assert_eq!(f.calls(), 2);
}

/// Un modelo del tamaño correcto pero dañado (no carga) se borra: la siguiente vez se descarga.
#[test]
fn test_unloadable_model_is_deleted_and_downloaded_again() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join(anotalo_core::models::TITANET.file);
    std::fs::File::create(&path)
        .unwrap()
        .set_len(anotalo_core::models::TITANET.size)
        .unwrap();
    let f = Arc::new(CountingFetch {
        data: b"no es el modelo".to_vec(),
        calls: AtomicUsize::new(0),
    });
    let m = SpeakerModel::new(d.path().to_path_buf(), f.clone());
    m.ensure();
    assert!(m.wait_idle(Duration::from_secs(30)));
    assert!(m.get().is_none());
    assert!(!path.exists(), "el modelo que no carga se borra");
    m.ensure();
    assert!(m.wait_idle(Duration::from_secs(30)));
    assert_eq!(f.calls(), 1, "y la siguiente vez se descarga");
}
