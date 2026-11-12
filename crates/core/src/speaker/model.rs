//! El modelo de voces de la app: se descarga (40 MB, una vez) y se carga en segundo plano, y se
//! queda cargado. Si no está listo, las llamadas se graban sin separar; si falla, se reintenta en
//! la siguiente llamada.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::embed::{Embed, Embedder};
use crate::models::{Fetch, ensure_titanet};

enum State {
    Idle,
    Loading,
    Ready(Arc<Embedder>),
}

pub struct SpeakerModel {
    dir: PathBuf,
    fetch: Arc<dyn Fetch + Send + Sync>,
    state: Mutex<State>,
    changed: Condvar,
}

impl SpeakerModel {
    pub fn new(dir: PathBuf, fetch: Arc<dyn Fetch + Send + Sync>) -> Arc<SpeakerModel> {
        Arc::new(SpeakerModel {
            dir,
            fetch,
            state: Mutex::new(State::Idle),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Empieza a descargar y cargar el modelo si no está listo ni en camino.
    pub fn ensure(self: &Arc<Self>) {
        {
            let mut st = self.lock();
            if !matches!(*st, State::Idle) {
                return;
            }
            *st = State::Loading;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("modelo de voces".into())
            .spawn(move || me.load());
        if spawned.is_err() {
            *self.lock() = State::Idle;
            self.changed.notify_all();
        }
    }

    fn load(&self) {
        let result = ensure_titanet(&self.dir, self.fetch.as_ref(), &mut |_| {}).and_then(|path| {
            // un archivo del tamaño correcto que no carga está dañado: se borra para descargarlo
            Embedder::load(&path).inspect_err(|_| {
                let _ = std::fs::remove_file(&path);
            })
        });
        let next = match result {
            Ok(e) => {
                log::info!("Modelo de voces listo");
                State::Ready(Arc::new(e))
            }
            Err(e) => {
                log::warn!("Modelo de voces no disponible: {e}");
                State::Idle
            }
        };
        *self.lock() = next;
        self.changed.notify_all();
    }

    pub fn get(&self) -> Option<Arc<dyn Embed>> {
        match &*self.lock() {
            State::Ready(e) => Some(e.clone() as Arc<dyn Embed>),
            _ => None,
        }
    }

    /// Espera a que no haya una carga en curso; false si se acabó el tiempo.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let st = self.lock();
        let (st, _) = self
            .changed
            .wait_timeout_while(st, timeout, |s| matches!(s, State::Loading))
            .unwrap_or_else(|e| e.into_inner());
        !matches!(*st, State::Loading)
    }
}
