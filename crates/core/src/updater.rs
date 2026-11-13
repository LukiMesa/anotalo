//! Actualizar la versión instalada desde las Releases públicas de GitHub (port de
//! `calltx/updater.py`, spec §2.1 "In-app updates" y §5.10).
//!
//! Se consulta la API de Releases (sin claves). Una versión estable lee `/releases/latest`; una de
//! pruebas (`1.0.0-alpha.N`) lee la lista, con las de pruebas incluidas, para pasar de una de
//! pruebas a la siguiente y luego a la estable. Solo vale `Anotalo-Setup-<ver>.exe` de
//! `LukiMesa/anotalo`, en su dirección exacta y con su huella SHA-256 publicada. Se descarga a un
//! `.part`, se comprueban tamaño y huella, y se lanza en silencio: el instalador cierra la app, la
//! actualiza y la vuelve a abrir. Nunca durante una grabación ni sin un clic.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::Settings;
use crate::models::Fetch;
use crate::notify::Notifier;

macro_rules! repo {
    () => {
        "LukiMesa/anotalo"
    };
}

/// El repositorio de las Releases.
pub const REPO: &str = repo!();
const API_LATEST: &str = concat!("https://api.github.com/repos/", repo!(), "/releases/latest");
const API_LIST: &str = concat!(
    "https://api.github.com/repos/",
    repo!(),
    "/releases?per_page=30"
);
const DOWNLOAD_BASE: &str = concat!("https://github.com/", repo!(), "/releases/download/");

/// Lo que se pasa al instalador: el contrato de Inno que también usa Python 0.6.x.
pub const INSTALLER_ARGS: [&str; 3] = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"];
/// Primera comprobación tras arrancar.
pub const FIRST_CHECK: Duration = Duration::from_secs(60);
/// Y después, cada…
pub const EVERY: Duration = Duration::from_secs(6 * 3600);
/// El instalador cierra la app en segundos: si sigue viva pasado este tiempo, algo falló.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Tope de la respuesta de la API (30 Releases con sus notas caben de sobra).
const MAX_JSON: u64 = 8 << 20;
const CHUNK: usize = 1 << 16;

const NOT_OFFICIAL: &str = "La descarga no viene de la página oficial de la app";
const INCOMPLETE: &str = "La descarga quedó incompleta";
const MISMATCH: &str = "El archivo descargado no coincide con el publicado";
const NOT_COMPLETED: &str = "La instalación no se completó. Vuelve a intentarlo.";

/// La API de Releases: la última estable, o la lista (con las de pruebas) para una versión de
/// pruebas.
pub fn api_url(prereleases: bool) -> &'static str {
    if prereleases { API_LIST } else { API_LATEST }
}

// --- versiones ---------------------------------------------------------------

/// Una versión SemVer 2.0 (`1.0.0`, `1.0.0-alpha.2`). El orden derivado es la precedencia de
/// SemVer: por campos, y `Pre` antes que `Stable`; los identificadores numéricos van antes que los
/// alfanuméricos y se comparan como números.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Pre,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Pre {
    Pre(Vec<Ident>),
    Stable,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    Num(u64),
    Alpha(String),
}

impl Version {
    /// SemVer con una `v` delante opcional. Los metadatos de compilación (`+…`) se comprueban y se
    /// ignoran. None si no es SemVer (`1.0`, `01.0.0`, `1.0.0-`).
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let (s, build) = match s.split_once('+') {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        if let Some(b) = build
            && !b
                .split('.')
                .all(|id| !id.is_empty() && id.bytes().all(ident_byte))
        {
            return None;
        }
        let (core, pre) = match s.split_once('-') {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        let nums: Vec<u64> = core.split('.').map(number).collect::<Option<_>>()?;
        let [major, minor, patch] = nums[..] else {
            return None;
        };
        let pre = match pre {
            None => Pre::Stable,
            Some(p) => Pre::Pre(p.split('.').map(ident).collect::<Option<_>>()?),
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// `1.0.0-alpha.1` sí; `1.0.0` no.
    pub fn is_prerelease(&self) -> bool {
        matches!(self.pre, Pre::Pre(_))
    }
}

fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// Un número sin ceros a la izquierda.
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

fn ident(s: &str) -> Option<Ident> {
    if s.is_empty() || !s.bytes().all(ident_byte) {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        number(s).map(Ident::Num)
    } else {
        Some(Ident::Alpha(s.to_string()))
    }
}

/// `remote` es posterior a `local`. False si alguna no es una versión.
pub fn is_newer(remote: &str, local: &str) -> bool {
    match (Version::parse(remote), Version::parse(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

// --- qué Release se toma -----------------------------------------------------

/// Una Release con su instalador.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// La etiqueta sin la `v`.
    pub version: String,
    pub title: String,
    pub url: String,
    pub name: String,
    pub size: u64,
    /// La huella publicada por GitHub, en hexadecimal en minúsculas.
    pub sha256: String,
}

/// El nombre del instalador de una versión.
pub fn asset_name(v: &str) -> String {
    format!("Anotalo-Setup-{v}.exe")
}

/// La dirección es exactamente la del instalador de esa versión en las Releases de Anotalo.
pub fn url_ok(url: &str, name: &str, version: &str) -> bool {
    Version::parse(version).is_some()
        && name == asset_name(version)
        && url == format!("{DOWNLOAD_BASE}v{version}/{name}")
}

/// La Release más nueva de la respuesta de la API (un objeto de `/latest` o la lista). Se saltan
/// los borradores, las de pruebas (salvo con `prereleases`), y las que no tienen
/// `Anotalo-Setup-<ver>.exe` con su huella `sha256:`.
pub fn pick_release(json: &Value, prereleases: bool) -> Option<Release> {
    let all: Vec<&Value> = match json {
        Value::Array(list) => list.iter().collect(),
        Value::Object(_) => vec![json],
        _ => vec![],
    };
    all.into_iter()
        .filter_map(|r| candidate(r, prereleases))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, rel)| rel)
}

fn candidate(r: &Value, prereleases: bool) -> Option<(Version, Release)> {
    if r.get("draft").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let tag = r.get("tag_name")?.as_str()?.trim();
    let version = tag.strip_prefix(['v', 'V']).unwrap_or(tag).to_string();
    let v = Version::parse(&version)?;
    let marked_pre = r
        .get("prerelease")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if (marked_pre || v.is_prerelease()) && !prereleases {
        return None;
    }
    let name = asset_name(&version);
    let asset = r
        .get("assets")?
        .as_array()?
        .iter()
        .find(|a| a.get("name").and_then(Value::as_str) == Some(name.as_str()))?;
    let digest = asset.get("digest")?.as_str()?.trim();
    let sha256 = digest
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("sha256:"))
        .map(|_| digest[7..].to_ascii_lowercase())
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let size = asset.get("size")?.as_u64()?;
    let url = asset.get("browser_download_url")?.as_str()?.to_string();
    let title = r
        .get("name")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(tag)
        .to_string();
    Some((
        v,
        Release {
            version,
            title,
            url,
            name,
            size,
            sha256,
        },
    ))
}

/// Pregunta a la API de GitHub. `Ok(None)`: no hay ninguna Release con instalador.
pub fn fetch_release(fetch: &dyn Fetch, prereleases: bool) -> Result<Option<Release>, String> {
    let mut body = Vec::new();
    fetch
        .open(api_url(prereleases), 0)
        .map_err(|e| e.to_string())?
        .take(MAX_JSON)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_slice(&body)
        .map_err(|e| format!("respuesta no válida de GitHub ({e})"))?;
    Ok(pick_release(&json, prereleases))
}

// --- descarga ----------------------------------------------------------------

/// Descarga el instalador a `dir` y comprueba que es el publicado (dirección, tamaño y huella). Si
/// algo no cuadra, borra lo descargado.
pub fn download(
    rel: &Release,
    dir: &Path,
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(f32),
) -> Result<PathBuf, String> {
    if !url_ok(&rel.url, &rel.name, &rel.version) {
        return Err(NOT_OFFICIAL.into());
    }
    let part = dir.join(format!("{}.part", rel.name));
    let done = dir.join(&rel.name);
    let result = fetch_to(rel, dir, &part, fetch, progress)
        .and_then(|()| fs::rename(&part, &done).map_err(DlError::Io));
    if let Err(e) = result {
        let _ = fs::remove_file(&part);
        return Err(match e {
            DlError::Incomplete => INCOMPLETE.into(),
            DlError::Mismatch => MISMATCH.into(),
            DlError::Io(e) => format!("No se pudo descargar: {e}"),
        });
    }
    progress(1.0);
    Ok(done)
}

enum DlError {
    Incomplete,
    Mismatch,
    Io(std::io::Error),
}

fn fetch_to(
    rel: &Release,
    dir: &Path,
    part: &Path,
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(f32),
) -> Result<(), DlError> {
    fs::create_dir_all(dir).map_err(DlError::Io)?;
    let mut reader = fetch.open(&rel.url, 0).map_err(DlError::Io)?;
    let mut out = File::create(part).map_err(DlError::Io)?;
    let mut hasher = Sha256::new();
    let mut have = 0u64;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).map_err(DlError::Io)?;
        if n == 0 {
            break;
        }
        have += n as u64;
        if have > rel.size {
            return Err(DlError::Mismatch);
        }
        out.write_all(&buf[..n]).map_err(DlError::Io)?;
        hasher.update(&buf[..n]);
        if rel.size > 0 {
            progress((have as f64 / rel.size as f64).min(1.0) as f32);
        }
    }
    out.sync_all().map_err(DlError::Io)?;
    if have != rel.size {
        return Err(DlError::Incomplete);
    }
    let sha: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if sha != rel.sha256.to_ascii_lowercase() {
        return Err(DlError::Mismatch);
    }
    Ok(())
}

/// `%TEMP%\Anotalo-update`: donde se descargan los instaladores.
pub fn download_dir() -> PathBuf {
    std::env::temp_dir().join("Anotalo-update")
}

/// Borra los instaladores ya usados (y la carpeta si queda vacía). Lo que esté en uso (el
/// instalador que acaba de abrir esta app) se queda para la próxima vez.
pub fn clear_downloads(dirs: &[PathBuf]) {
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_file())
                && let Err(err) = fs::remove_file(e.path())
            {
                log::debug!("No se pudo borrar {} ({err})", e.path().display());
            }
        }
        let _ = fs::remove_dir(dir);
    }
}

// --- el estado para la ventana -----------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    Downloading,
    /// Hay que instalar, pero se está grabando: se instala al terminar.
    Waiting,
    Installing,
    Error,
    /// Ya es la última (solo cuando lo pidió el usuario).
    Latest,
}

impl UpdateState {
    /// Los mismos textos que la app de Python (la ventana los usa tal cual).
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateState::Idle => "idle",
            UpdateState::Checking => "checking",
            UpdateState::Available => "available",
            UpdateState::Downloading => "downloading",
            UpdateState::Waiting => "waiting",
            UpdateState::Installing => "installing",
            UpdateState::Error => "error",
            UpdateState::Latest => "latest",
        }
    }
}

pub type FetchFn = Box<dyn Fn() -> Result<Option<Release>, String> + Send + Sync>;
pub type DownloadFn =
    Box<dyn Fn(&Release, &mut dyn FnMut(f32)) -> Result<PathBuf, String> + Send + Sync>;
pub type LaunchFn = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// Lo que necesita `UpdateManager` del resto de la app (en las pruebas, cierres de mentira).
/// Ninguno de estos cierres debe llamar a `check`, `install` u `on_recording_end`: se llaman con
/// las transiciones en curso. `snapshot` sí se puede llamar.
pub struct UpdateDeps {
    /// La versión de esta app.
    pub current: String,
    pub is_recording: Box<dyn Fn() -> bool + Send + Sync>,
    pub notifier: Arc<dyn Notifier>,
    /// Cada cambio, con la foto nueva (`snapshot`).
    pub on_change: Box<dyn Fn(Value) + Send + Sync>,
    /// true justo antes de la última comprobación de `is_recording` (desde ahí no se pueden empezar
    /// grabaciones); false si había una (se espera) o si la instalación falla. Debe esperar a que
    /// termine de empezar una grabación en curso (`App::set_updating` toma el turno de acciones).
    pub set_updating: Box<dyn Fn(bool) + Send + Sync>,
    pub fetch: FetchFn,
    pub download: DownloadFn,
    /// Lanza el instalador descargado (en silencio y suelto).
    pub launch: LaunchFn,
    /// La versión de la que ya se avisó (ajuste `update_notified`).
    pub notified_version: String,
    /// Se avisó de una versión nueva: se guarda en los ajustes.
    pub on_notified: Box<dyn Fn(&str) + Send + Sync>,
    pub install_timeout: Duration,
}

struct Data {
    state: UpdateState,
    release: Option<Release>,
    progress: Option<f32>,
    error: Option<String>,
    /// El instalador ya descargado y comprobado.
    path: Option<PathBuf>,
    notified: String,
}

/// El estado de la actualización (`calltx/updater.py::UpdateManager`). Las llamadas son síncronas:
/// la app las lanza en hilos aparte.
pub struct UpdateManager {
    d: UpdateDeps,
    /// Las transiciones, de una en una (el `_lock` de Python). Se tiene al preguntar si se graba y
    /// al bloquear las grabaciones; nunca mientras se comprueba o se descarga.
    ops: Mutex<()>,
    /// Lo que enseña `snapshot`. Nunca se tiene mientras se llama a otro código.
    data: Mutex<Data>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    installs: AtomicU64,
    me: Weak<UpdateManager>,
}

impl UpdateManager {
    pub fn new(d: UpdateDeps) -> Arc<UpdateManager> {
        Arc::new_cyclic(|me| UpdateManager {
            data: Mutex::new(Data {
                state: UpdateState::Idle,
                release: None,
                progress: None,
                error: None,
                path: None,
                notified: d.notified_version.clone(),
            }),
            d,
            ops: Mutex::new(()),
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            installs: AtomicU64::new(0),
            me: me.clone(),
        })
    }

    /// `{"state","version","title","progress","error","current"}`, como Python.
    pub fn snapshot(&self) -> Value {
        snapshot_of(&self.data(), &self.d.current)
    }

    pub fn state(&self) -> UpdateState {
        self.data().state
    }

    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ops(&self) -> MutexGuard<'_, ()> {
        self.ops.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, state: UpdateState, error: Option<String>) {
        let snap = {
            let mut d = self.data();
            d.state = state;
            d.error = error;
            snapshot_of(&d, &self.d.current)
        };
        (self.d.on_change)(snap);
    }

    // --- comprobar ---

    /// Pregunta si hay una versión nueva. `user`: lo pidió el usuario ("Buscar actualizaciones"),
    /// y entonces se enseñan los errores y "ya es la última".
    pub fn check(&self, user: bool) {
        let prev = {
            let _ops = self.ops();
            let prev = self.state();
            if matches!(
                prev,
                UpdateState::Downloading | UpdateState::Waiting | UpdateState::Installing
            ) {
                return;
            }
            self.set(UpdateState::Checking, None);
            prev
        };
        let result = (self.d.fetch)();
        let announce = {
            let _ops = self.ops();
            match result {
                Err(e) => {
                    log::info!("No se pudo comprobar si hay actualizaciones: {e}");
                    if user {
                        self.set(
                            UpdateState::Error,
                            Some(format!("No se pudo comprobar: {e}")),
                        );
                    } else if matches!(prev, UpdateState::Available | UpdateState::Latest) {
                        self.set(prev, None);
                    } else {
                        self.set(UpdateState::Idle, None);
                    }
                    None
                }
                Ok(Some(rel)) if is_newer(&rel.version, &self.d.current) => {
                    let first_time = {
                        let mut d = self.data();
                        if d.release.as_ref() != Some(&rel) {
                            d.path = None;
                        }
                        d.release = Some(rel.clone());
                        let first = d.notified != rel.version;
                        if first {
                            d.notified = rel.version.clone();
                        }
                        first
                    };
                    self.set(UpdateState::Available, None);
                    first_time.then_some(rel.version)
                }
                Ok(_) => {
                    let state = if user {
                        UpdateState::Latest
                    } else {
                        UpdateState::Idle
                    };
                    self.set(state, None);
                    None
                }
            }
        };
        if let Some(v) = announce {
            log::info!("Hay una versión nueva: {v}");
            self.d.notifier.notify(
                "Actualización disponible",
                &format!("Versión {v} lista para instalar desde la app"),
                None,
            );
            (self.d.on_notified)(&v);
        }
    }

    // --- instalar ---

    /// El clic en "Instalar": descarga e instala ahora, o al terminar la grabación en curso.
    pub fn install(&self) {
        {
            let _ops = self.ops();
            let (state, has_release) = {
                let d = self.data();
                (d.state, d.release.is_some())
            };
            if !has_release || !matches!(state, UpdateState::Available | UpdateState::Error) {
                return;
            }
            if (self.d.is_recording)() {
                self.set(UpdateState::Waiting, None);
                return;
            }
            self.set(UpdateState::Downloading, None);
        }
        self.run();
    }

    /// Terminó una grabación: si había una instalación esperando, sigue.
    pub fn on_recording_end(&self) {
        {
            let _ops = self.ops();
            if self.state() != UpdateState::Waiting {
                return;
            }
            self.set(UpdateState::Downloading, None);
        }
        self.run();
    }

    fn run(&self) {
        let (release, path) = {
            let d = self.data();
            (d.release.clone(), d.path.clone())
        };
        let Some(rel) = release else {
            return;
        };
        let path = match path {
            Some(p) => p,
            None => {
                self.data().progress = Some(0.0);
                match (self.d.download)(&rel, &mut |p| self.progress(p)) {
                    Ok(p) => {
                        self.data().path = Some(p.clone());
                        p
                    }
                    Err(e) => {
                        let _ops = self.ops();
                        self.fail(e);
                        return;
                    }
                }
            }
        };
        {
            let _ops = self.ops();
            // La descarga tarda: puede haber empezado otra grabación mientras tanto. Primero se
            // bloquean las grabaciones y luego se mira: así ninguna empieza entre medias.
            (self.d.set_updating)(true);
            if (self.d.is_recording)() {
                (self.d.set_updating)(false);
                self.set(UpdateState::Waiting, None);
                return;
            }
            self.set(UpdateState::Installing, None);
        }
        log::info!("Instalando la versión {}", rel.version);
        match (self.d.launch)(&path) {
            Ok(()) => self.arm_timeout(),
            Err(e) => {
                let _ops = self.ops();
                self.fail(e);
                (self.d.set_updating)(false);
            }
        }
    }

    fn fail(&self, e: String) {
        log::warn!("No se pudo actualizar: {e}");
        {
            let mut d = self.data();
            d.progress = None;
            d.path = None;
        }
        self.set(UpdateState::Error, Some(e));
    }

    /// El instalador cierra esta app en segundos. Si sigue viva al rato, algo falló.
    fn arm_timeout(&self) {
        let n = self.installs.fetch_add(1, Ordering::SeqCst) + 1;
        let me = self.me.clone();
        let wait = self.d.install_timeout;
        let spawned = std::thread::Builder::new()
            .name("actualización: espera".into())
            .spawn(move || {
                std::thread::sleep(wait);
                if let Some(m) = me.upgrade() {
                    m.expire(n);
                }
            });
        if let Err(e) = spawned {
            log::warn!("No se pudo vigilar la instalación ({e})");
        }
    }

    fn expire(&self, n: u64) {
        let _ops = self.ops();
        if self.state() != UpdateState::Installing || self.installs.load(Ordering::SeqCst) != n {
            return;
        }
        log::warn!(
            "La instalación no cerró la app en {:?}",
            self.d.install_timeout
        );
        self.data().path = None;
        self.set(UpdateState::Error, Some(NOT_COMPLETED.into()));
        (self.d.set_updating)(false);
    }

    fn progress(&self, p: f32) {
        let snap = {
            let mut d = self.data();
            let pct = |x: f32| (x * 100.0) as i32;
            if d.progress.is_some_and(|old| pct(old) == pct(p)) {
                return;
            }
            d.progress = Some(p);
            snapshot_of(&d, &self.d.current)
        };
        (self.d.on_change)(snap);
    }

    // --- en segundo plano ---

    /// Comprueba pasado `first` y luego cada `every`, mientras `enabled` (el ajuste "Buscar
    /// actualizaciones automáticamente") esté activado; si no, esa vez se salta.
    pub fn start_background(
        self: &Arc<Self>,
        first: Duration,
        every: Duration,
        enabled: Arc<AtomicBool>,
    ) {
        let me = Arc::downgrade(self);
        let stop = self.stop.clone();
        let spawned = std::thread::Builder::new()
            .name("actualizaciones".into())
            .spawn(move || {
                let mut delay = first;
                loop {
                    if wait_or_stop(&stop, delay) {
                        return;
                    }
                    let Some(m) = me.upgrade() else {
                        return;
                    };
                    if enabled.load(Ordering::SeqCst) {
                        let checked =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                m.check(false)
                            }));
                        if checked.is_err() {
                            log::error!("Error comprobando actualizaciones");
                        }
                    }
                    delay = every;
                }
            });
        if let Err(e) = spawned {
            log::warn!("No se pudo empezar a buscar actualizaciones ({e})");
        }
    }

    /// Deja de comprobar en segundo plano.
    pub fn stop(&self) {
        let (lock, cv) = &*self.stop;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

/// Espera `delay` o hasta `stop`. true si se paró.
fn wait_or_stop(stop: &(Mutex<bool>, Condvar), delay: Duration) -> bool {
    let (lock, cv) = stop;
    let deadline = Instant::now() + delay;
    let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !*stopped {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        stopped = cv
            .wait_timeout(stopped, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    *stopped
}

fn snapshot_of(d: &Data, current: &str) -> Value {
    json!({
        "state": d.state.as_str(),
        "version": d.release.as_ref().map(|r| r.version.as_str()),
        "title": d.release.as_ref().map(|r| r.title.as_str()),
        "progress": d.progress,
        "error": d.error,
        "current": current,
    })
}

/// Tras actualizar, avisa una vez de la versión nueva. Devuelve true si cambió `s` (hay que
/// guardar). La primera vez solo se apunta la versión; también al llegar desde Transcriptor.
pub fn announce_if_updated(s: &mut Settings, version: &str, n: &dyn Notifier) -> bool {
    if s.last_run_version == version {
        return false;
    }
    let prev = std::mem::replace(&mut s.last_run_version, version.to_string());
    if !prev.is_empty() && !from_transcriptor(&prev, version) {
        n.notify(
            "Anotalo actualizado",
            &format!("Ya tienes la versión {version}"),
            None,
        );
    }
    true
}

/// De una versión 0.x a una 1.x o posterior: las 0.x son de Transcriptor (la app de Python), cuyos
/// ajustes se copian con su `last_run_version`. Pasar a Anotalo no es "Anotalo actualizado".
fn from_transcriptor(prev: &str, version: &str) -> bool {
    matches!(
        (Version::parse(prev), Version::parse(version)),
        (Some(p), Some(v)) if p.major == 0 && v.major >= 1
    )
}
