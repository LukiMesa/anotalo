//! Fuentes de audio: lo que la sesión necesita de un micrófono o de "lo que suena".

use std::sync::OnceLock;
use std::time::Instant;

use crate::routing::Audio;

/// Recibe (instante de la primera muestra según [`now_s`], muestras mono a 16 kHz).
pub type OnAudio = Box<dyn FnMut(f64, &[f32]) + Send>;

/// Una fuente que entrega audio mono a 16 kHz desde su propio hilo.
pub trait AudioSource: Send {
    fn name(&self) -> &str;
    fn start(&mut self, on_audio: OnAudio) -> Result<(), String>;
    fn stop(&mut self);
    /// false si la captura terminó sola (dispositivo desconectado, programa cerrado…).
    fn is_active(&self) -> bool;
}

/// Segundos monotónicos desde que el programa lo consultó por primera vez: el reloj común de las
/// fuentes y la sesión.
pub fn now_s() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// De dónde sale la voz de los demás: el audio de un programa o el dispositivo donde suena.
#[derive(Debug, Clone, PartialEq)]
pub struct OthersSpec {
    pub kind: Audio,
    /// Exe del programa (para `Audio::Process`).
    pub exe: Option<String>,
    /// Plataforma de la llamada (None = grabada a mano).
    pub platform: Option<String>,
}
