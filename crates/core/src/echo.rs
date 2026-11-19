//! Cancelación de eco: sin auriculares, el micrófono oye por los altavoces lo que dicen los demás y
//! "Yo" repetiría sus frases. Lo que suena (el audio de "Otros", que ya se captura) es la
//! referencia: el AEC3 de WebRTC (crate `sonora`) lo resta del micrófono antes del segmentador.
//!
//! Todo se coloca en una línea de tiempo común, en muestras de 16 kHz del reloj de [`now_s`]
//! (índice = instante × 16 000):
//!
//! - [`FarBuffer`]: lo que sonó, cada trozo en su instante. La captura del sistema no manda nada
//!   mientras la llamada calla: el hueco es silencio. Los instantes llegan con algo de jitter (el
//!   remuestreador entrega a tramos de ~21 ms): dentro de [`JITTER_S`] el audio sigue seguido.
//! - [`EchoCanceller`]: el micrófono, en tramas de 10 ms. Con cada trama de instante `t` el AEC
//!   recibe lo que sonó en `t + LEAD_S` (lo que sonó va siempre por delante; el AEC3 estima solo el
//!   retardo que queda, el de la salida de audio, la sala y la entrada). Si eso aún no llegó, la
//!   trama espera; pasado [`MAX_WAIT_S`] se da por silencio y sigue.
//!
//! La salida conserva los trozos del micrófono tal cual entraron (mismos instantes y tamaños, con
//! el retardo interno del AEC compensado): las líneas de "Yo" no se mueven. Si el AEC falla, se
//! avisa una vez y sigue el micrófono sin limpiar, sin perder ni desplazar una muestra.
//!
//! [`now_s`]: crate::source::now_s

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use sonora::config::{EchoCanceller as Aec3Config, TransparentModeType};
use sonora::{AudioProcessing, Config, StreamConfig};

pub const SR: usize = 16_000;
/// Trama del AEC: 10 ms.
pub const FRAME: usize = 160;
/// Lo que sonó va por delante del micrófono: con la trama del micrófono de `t` entra lo que sonó
/// hasta `t + LEAD_S`. El eco real llega después de sonar (salida de audio + sala + entrada: de
/// 20 ms con cable a 250 ms con Bluetooth), pero los instantes de las dos capturas pueden errar
/// hasta [`JITTER_S`] cada uno: este margen mantiene el eco por detrás de su referencia. Con él, el
/// retardo que ve el AEC3 queda dentro de lo que busca (~600 ms) y el micrófono se retiene ~60 ms.
pub const LEAD_S: f64 = 0.05;
/// Cuánto espera una trama del micrófono a lo que sonó con ella antes de darlo por silencio: más
/// que el búfer de WASAPI (200 ms), lo más que puede llegar tarde un paquete sin perderse. Es el
/// retraso máximo que añade al micrófono cuando la llamada calla.
pub const MAX_WAIT_S: f64 = 0.3;
/// Desfase entre lo que dice el instante de un trozo y lo que va sumando su audio que aún se toma
/// por jitter (el remuestreador entrega a tramos de ~21 ms y el hilo despierta tarde): por debajo,
/// el audio sigue seguido; por encima, es un hueco (o algo que llegó tarde) y se recoloca.
pub const JITTER_S: f64 = 0.05;
/// Lo más que se guarda de lo que sonó (5 s, 320 KB): de sobra para un micrófono que va algo por
/// detrás; si el micrófono deja de leerlo, no crece.
pub const FAR_CAP_S: f64 = 5.0;
/// Tras un hueco, lo que vuelve a sonar se junta este rato antes de colocarlo (ver
/// [`FarBuffer::push`]): ~10 trozos, más que un ciclo del diente de sierra del remuestreador
/// (160 ms). El micrófono lo espera (cabe en [`MAX_WAIT_S`]).
pub const RESUME_HOLD_S: f64 = 0.2;

const JITTER: i64 = (JITTER_S * SR as f64) as i64;
const LEAD: i64 = (LEAD_S * SR as f64) as i64;
const FAR_CAP: usize = (FAR_CAP_S * SR as f64) as usize;
const RESUME_HOLD: usize = (RESUME_HOLD_S * SR as f64) as usize;
/// Peso de cada ventana nueva en el suelo de los retrasos (ver `FarBuffer::floor`): promedia unas
/// 10 ventanas (2 s), así que sigue la deriva de un reloj (100 ppm = 1,6 muestras por segundo) con
/// unas 3 muestras de retraso.
const FLOOR_WEIGHT: f64 = 0.1;
/// La salida del AEC3 va 128 muestras (8 ms) por detrás de la entrada: un bloque de 64 al
/// reagrupar las tramas de 10 ms y otro en su filtro de supresión (solapa bloques de 64). Medido:
/// sin nada que cancelar, la salida es la entrada pasada por su paso alto, 128 muestras después.
const AEC3_LATENCY: usize = 128;

fn index(t: f64) -> i64 {
    (t * SR as f64).round() as i64
}

fn secs(i: i64) -> f64 {
    i as f64 / SR as f64
}

/// Lo que sonó, compartido: lo llena el callback de "Otros" y lo lee el carril del micrófono.
pub type SharedFar = Arc<Mutex<FarBuffer>>;

/// Lo que sonó, cada muestra en su instante (ver el módulo).
#[derive(Default)]
pub struct FarBuffer {
    samples: VecDeque<f32>,
    /// Índice de `samples[0]`; None hasta que llega algo.
    start: Option<i64>,
    /// El suelo de los retrasos: lo que dice el instante del trozo menos tardío de cada ventana de
    /// [`RESUME_HOLD_S`] respecto a donde quedó su audio (≤ 0: el primero fija el sitio y los menos
    /// tardíos dicen antes), promediado (sigue la deriva del reloj del dispositivo). None hasta la
    /// primera ventana.
    floor: Option<f64>,
    /// La ventana en curso: su mínimo y cuántas muestras lleva.
    window: (i64, usize),
    /// Lo que volvió a sonar tras un hueco, aún sin colocar: (índice según su instante, audio).
    resumed: Vec<(i64, Vec<f32>)>,
    /// Dónde empieza audio recolocado (tras un hueco o lo que llegó tarde): ahí lo que sonó puede
    /// haberse movido unas muestras respecto al eco ya aprendido.
    moved: VecDeque<i64>,
}

impl FarBuffer {
    pub fn new() -> FarBuffer {
        FarBuffer::default()
    }

    pub fn shared() -> SharedFar {
        Arc::new(Mutex::new(FarBuffer::new()))
    }

    fn end(&self) -> Option<i64> {
        self.start.map(|s| s + self.samples.len() as i64)
    }

    /// Añade un trozo que empieza en `t`. Solo copia: el callback de audio no espera.
    ///
    /// Seguido de lo anterior salvo que el instante diga otra cosa por más de [`JITTER_S`] (respecto
    /// a lo tarde que suelen llegar, ver `floor`). Si dice después, la llamada calló (sin paquetes):
    /// ese rato sonó silencio, y lo que vuelve a sonar se junta [`RESUME_HOLD_S`] antes de colocarlo
    /// (ver `resume`). Si dice antes, llega tarde (ya se dio por silencio): lo que cae antes de lo
    /// que ya hay sobra. La deriva del reloj del dispositivo se corrige de una vez al pasar de
    /// [`JITTER_S`].
    pub fn push(&mut self, t: f64, x: &[f32]) {
        if x.is_empty() {
            return;
        }
        let at = index(t);
        let Some(end) = self.end() else {
            self.start = Some(at);
            self.window = (0, x.len());
            self.append(x);
            return;
        };
        if let Some((first, _)) = self.resumed.first() {
            let held: usize = self.resumed.iter().map(|(_, x)| x.len()).sum();
            if at - (first + held as i64) > JITTER {
                // volvió a callar antes de juntar bastante: va lo que hay y empieza otra tanda
                self.resume();
                self.push(t, x);
                return;
            }
            self.resumed.push((at, x.to_vec()));
            if held + x.len() >= RESUME_HOLD {
                self.resume();
            }
            return;
        }
        let gap = at - end;
        let usual = self.floor.unwrap_or(0.0).round() as i64;
        if gap - usual > JITTER {
            self.resumed.push((at, x.to_vec()));
            if x.len() >= RESUME_HOLD {
                self.resume();
            }
        } else if gap - usual < -JITTER {
            let late = (usual - gap) as usize;
            if late < x.len() {
                self.moved.push_back(end);
                self.append(&x[late..]);
            }
        } else {
            let (low, len) = self.window;
            self.window = (low.min(gap), len + x.len());
            if self.window.1 >= RESUME_HOLD {
                let low = self.window.0 as f64;
                self.floor = Some(self.floor.map_or(low, |f| f + FLOOR_WEIGHT * (low - f)));
                self.window = (i64::MAX, 0);
            }
            self.drift(end);
            self.append(x);
        }
    }

    /// El reloj del dispositivo no va igual que el del sistema (unas decenas de ppm): su audio,
    /// seguido, se va separando de sus instantes y el suelo lo sigue. Al pasar de [`JITTER_S`] se
    /// recoloca de una vez (silencio si va atrasado, se salta audio si va adelantado), justo antes
    /// de lo que llega: con 100 ppm, una vez cada ~8 min.
    fn drift(&mut self, end: i64) {
        let Some(floor) = self.floor else {
            return;
        };
        let shift = floor.round() as i64;
        if shift.abs() <= JITTER {
            return;
        }
        if shift > 0 {
            self.samples
                .extend(std::iter::repeat_n(0.0, shift as usize));
        } else {
            let cut = (shift.unsigned_abs() as usize).min(self.samples.len());
            self.samples.truncate(self.samples.len() - cut);
        }
        self.floor = Some(floor - shift as f64);
        self.window = (i64::MAX, 0);
        self.moved.push_back(end.min(end + shift));
    }

    /// Coloca lo que volvió a sonar tras un hueco. Cada instante llega tarde al azar (unos ms): con
    /// el de un solo trozo, lo que sonó se movería respecto a antes del hueco y el AEC3 tendría que
    /// volver a aprender el eco (~1 s con el eco sin quitar). Con el trozo menos tardío de la tanda
    /// y el suelo de antes del hueco (medido igual, en ventanas del mismo tamaño), queda donde
    /// estaría si la llamada no hubiera callado.
    fn resume(&mut self) {
        let held = std::mem::take(&mut self.resumed);
        let mut offset = 0i64;
        let mut first = i64::MAX;
        for (at, x) in &held {
            first = first.min(at - offset);
            offset += x.len() as i64;
        }
        let floor = self.floor.unwrap_or(self.window.0.min(0) as f64);
        self.window = (i64::MAX, 0);
        let at = first - floor.round() as i64;
        let x: Vec<f32> = held.into_iter().flat_map(|(_, x)| x).collect();
        let end = self.end().unwrap_or(at);
        self.moved.push_back(at.max(end));
        let gap = at - end;
        if gap >= 0 {
            if gap as usize >= FAR_CAP {
                self.samples.clear();
                self.start = Some(at);
            } else {
                self.samples.extend(std::iter::repeat_n(0.0, gap as usize));
            }
            self.append(&x);
        } else if (gap.unsigned_abs() as usize) < x.len() {
            self.append(&x[gap.unsigned_abs() as usize..]);
        }
    }

    fn append(&mut self, x: &[f32]) {
        self.samples.extend(x.iter().copied());
        let excess = self.samples.len().saturating_sub(FAR_CAP);
        if excess > 0 {
            self.samples.drain(..excess);
            self.start = self.start.map(|s| s + excess as i64);
            // si nadie lee (el micrófono no está), tampoco crecen las marcas
            let start = self.start.unwrap_or(i64::MIN);
            while self.moved.front().is_some_and(|m| *m < start) {
                self.moved.pop_front();
            }
        }
    }

    /// Hasta qué instante se sabe lo que sonó (None si aún no llegó nada).
    pub fn end_s(&self) -> Option<f64> {
        self.end().map(secs)
    }

    /// Muestras guardadas.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Copia lo que sonó desde `t` (silencio donde no hay nada) y olvida lo anterior.
    pub fn read(&mut self, t: f64, out: &mut [f32]) {
        self.read_at(index(t), out);
    }

    /// Como `read`; true si en lo leído empieza audio recolocado.
    fn read_at(&mut self, from: i64, out: &mut [f32]) -> bool {
        let to = from + out.len() as i64;
        let mut moved = false;
        while self.moved.front().is_some_and(|m| *m < to) {
            self.moved.pop_front();
            moved = true;
        }
        self.copy(from, out);
        moved
    }

    fn copy(&mut self, from: i64, out: &mut [f32]) {
        out.fill(0.0);
        let Some(start) = self.start else {
            return;
        };
        if from > start {
            let old = ((from - start) as usize).min(self.samples.len());
            self.samples.drain(..old);
            // si el lector va por delante (lo que faltaba se dio por silencio), lo que llegue
            // después con instantes anteriores ya no sirve
            self.start = Some(from);
        }
        let start = self.start.unwrap_or(from);
        let skip = (start - from).max(0) as usize;
        if skip >= out.len() {
            return;
        }
        for (o, v) in out[skip..].iter_mut().zip(self.samples.iter()) {
            *o = *v;
        }
    }
}

/// El cancelador por dentro: tramas de [`FRAME`] muestras de lo que suena y del micrófono.
pub trait EchoProcessor: Send {
    /// Lo que suena ahora.
    fn render(&mut self, frame: &[f32]) -> Result<(), String>;
    /// Limpia una trama del micrófono.
    fn capture(&mut self, frame: &[f32], out: &mut [f32]) -> Result<(), String>;
    /// Muestras que la salida va por detrás de la entrada (como mucho [`FRAME`]).
    fn latency(&self) -> usize;
    /// Lo que suena se movió unas muestras respecto al eco ya aprendido (tras un hueco).
    fn echo_path_changed(&mut self) {}
}

/// El AEC3 de WebRTC, solo el cancelador de eco: sin supresión de ruido ni control de ganancia
/// (Whisper transcribe mejor la voz sin procesar). Lleva su filtro paso alto (~100 Hz), que el AEC3
/// necesita: sin nada que cancelar, la salida es la entrada filtrada (−0,3 dB en voz).
struct Aec3 {
    apm: AudioProcessing,
    /// Uno nuevo que toma el relevo en la próxima trama (ver `echo_path_changed`).
    fresh: Option<AudioProcessing>,
    scratch: Vec<f32>,
}

fn new_apm() -> AudioProcessing {
    let stream = StreamConfig::new(SR as u32, 1);
    AudioProcessing::builder()
        .config(Config {
            // Con auriculares no hay eco: el modo transparente lo detecta y deja de suprimir. El de
            // HMM lo hace en ~3 s con los dos hablando a la vez (el de contadores, en ~10 s), igual
            // de bien con eco.
            echo_canceller: Some(Aec3Config {
                transparent_mode: TransparentModeType::Hmm,
                ..Aec3Config::default()
            }),
            ..Default::default()
        })
        .capture_config(stream)
        .render_config(stream)
        .build()
}

impl Aec3 {
    fn new() -> Aec3 {
        Aec3 {
            apm: new_apm(),
            fresh: None,
            scratch: vec![0.0; FRAME],
        }
    }
}

impl EchoProcessor for Aec3 {
    fn render(&mut self, frame: &[f32]) -> Result<(), String> {
        if let Some(fresh) = self.fresh.as_mut() {
            fresh
                .process_render_f32(&[frame], &mut [&mut self.scratch[..]])
                .map_err(|e| e.to_string())?;
        }
        self.apm
            .process_render_f32(&[frame], &mut [&mut self.scratch[..]])
            .map_err(|e| e.to_string())
    }

    fn capture(&mut self, frame: &[f32], out: &mut [f32]) -> Result<(), String> {
        self.apm
            .process_capture_f32(&[frame], &mut [out])
            .map_err(|e| e.to_string())?;
        if let Some(mut fresh) = self.fresh.take() {
            // lo que el viejo tenía dentro sale de él; lo demás, del nuevo
            let mut tail = [0.0f32; FRAME];
            fresh
                .process_capture_f32(&[frame], &mut [&mut tail[..]])
                .map_err(|e| e.to_string())?;
            out[AEC3_LATENCY..].copy_from_slice(&tail[AEC3_LATENCY..]);
            self.apm = fresh;
        }
        Ok(())
    }

    fn latency(&self) -> usize {
        AEC3_LATENCY
    }

    /// Basta un desplazamiento de unas muestras (a 2 kHz, 4 muestras son media onda) para que el
    /// filtro aprendido reste mal; el AEC3 tarda en notarlo y mientras tanto se fía de él y deja
    /// pasar eco, a veces segundos. Uno nuevo empieza prudente, como al principio de la llamada:
    /// medido tras 2 s sin paquetes, 22–35 dB de ERLE el primer segundo y ~38 el siguiente, frente
    /// a 5–45 (y caídas segundos después) siguiendo con el mismo.
    fn echo_path_changed(&mut self) {
        self.fresh = Some(new_apm());
    }
}

/// Un trozo del micrófono tal como entró, y lo que ya se limpió de él.
struct Chunk {
    t: f64,
    /// Índice de su primera muestra en la línea de tiempo del micrófono.
    at: i64,
    raw: Vec<f32>,
    clean: Vec<f32>,
}

/// Quita del micrófono lo que sonó por los altavoces (ver el módulo). Lo usa el carril del
/// micrófono; lo que sonó lo llena el de "Otros" en el [`FarBuffer`] compartido.
pub struct EchoCanceller {
    /// None: el AEC falló y el micrófono pasa tal cual.
    aec: Option<Box<dyn EchoProcessor>>,
    far: SharedFar,
    /// Lo recibido y aún no devuelto, en orden.
    chunks: VecDeque<Chunk>,
    /// Siguiente muestra sin procesar: (trozo de `chunks`, posición dentro).
    next: (usize, usize),
    /// Muestras recibidas sin procesar.
    pending: usize,
    /// Fin de lo recibido en la línea de tiempo del micrófono.
    mic_end: Option<i64>,
    /// Desde dónde se recolocó el micrófono (se reabrió o su reloj derivó): ahí cambia respecto a
    /// lo que suena, como al recolocar lo que sonó.
    mic_moved: Option<i64>,
    /// Índice de la última trama procesada.
    last_at: i64,
    latency: usize,
    /// Salida que aún hay que descartar: la que el AEC da antes de la primera muestra.
    skip: usize,
    /// Las últimas `latency` muestras crudas: si el AEC falla, ocupan el lugar de lo que tenía
    /// dentro.
    tail: Vec<f32>,
    frame: Vec<f32>,
    render: Vec<f32>,
    out: Vec<f32>,
}

impl EchoCanceller {
    /// Con el AEC3. Si no se puede crear, el micrófono pasa tal cual.
    pub fn new(far: SharedFar) -> EchoCanceller {
        let aec: Box<dyn EchoProcessor> = match catch_unwind(Aec3::new) {
            Ok(aec) => Box::new(aec),
            Err(_) => {
                log::error!("No se pudo crear la cancelación de eco; el micrófono sigue sin ella");
                Box::new(Bypass)
            }
        };
        EchoCanceller::with_processor(far, aec)
    }

    /// Con otro cancelador (las pruebas).
    pub fn with_processor(far: SharedFar, aec: Box<dyn EchoProcessor>) -> EchoCanceller {
        let latency = aec.latency().min(FRAME);
        EchoCanceller {
            aec: Some(aec),
            far,
            chunks: VecDeque::new(),
            next: (0, 0),
            pending: 0,
            mic_end: None,
            mic_moved: None,
            last_at: 0,
            latency,
            skip: latency,
            tail: vec![0.0; latency],
            frame: vec![0.0; FRAME],
            render: vec![0.0; FRAME],
            out: vec![0.0; FRAME],
        }
    }

    /// Recibe un trozo del micrófono que empieza en `t` y devuelve los trozos ya limpios, cada uno
    /// con su instante. `now`: la hora ([`now_s`](crate::source::now_s)), para los plazos.
    pub fn process(&mut self, t: f64, x: &[f32], now: f64) -> Vec<(f64, Vec<f32>)> {
        if !x.is_empty() {
            let i = index(t);
            // seguido salvo hueco (o salto atrás) de verdad: el micrófono no calla nunca
            let at = match self.mic_end {
                Some(end) if (i - end).abs() <= JITTER => end,
                Some(_) => {
                    self.mic_moved = Some(i);
                    i
                }
                None => i,
            };
            self.mic_end = Some(at + x.len() as i64);
            self.pending += x.len();
            self.chunks.push_back(Chunk {
                t,
                at,
                raw: x.to_vec(),
                clean: Vec::with_capacity(x.len()),
            });
        }
        self.run(now, false)
    }

    /// Sin audio nuevo (el reloj de la sesión): limpia lo que ya se pueda.
    pub fn poll(&mut self, now: f64) -> Vec<(f64, Vec<f32>)> {
        self.run(now, false)
    }

    /// Fin de la sesión: todo lo pendiente, sin esperar (lo que no sonó aún, silencio).
    pub fn drain(&mut self) -> Vec<(f64, Vec<f32>)> {
        self.run(f64::INFINITY, true)
    }

    /// Instante del primer trozo retenido: el segmentador ya tiene todo lo anterior.
    pub fn held_since(&self) -> Option<f64> {
        self.chunks.front().map(|c| c.t)
    }

    /// Lo recibido y aún no devuelto, sin limpiar.
    pub fn into_raw(self) -> Vec<(f64, Vec<f32>)> {
        self.chunks.into_iter().map(|c| (c.t, c.raw)).collect()
    }

    fn run(&mut self, now: f64, drain: bool) -> Vec<(f64, Vec<f32>)> {
        let mut done = Vec::new();
        // al vaciar, tramas de relleno hasta que salga lo que el AEC aún tiene dentro
        while !self.chunks.is_empty() && (drain || self.pending >= FRAME) {
            let at = match self.chunks.get(self.next.0) {
                Some(c) => c.at + self.next.1 as i64,
                None => self.last_at + FRAME as i64,
            };
            let far_from = at + LEAD;
            let far_to = far_from + FRAME as i64;
            let mut moved = {
                let mut far = self.far.lock().unwrap_or_else(|e| e.into_inner());
                if !drain
                    && far.end().is_none_or(|end| end < far_to)
                    && now < secs(far_to) + MAX_WAIT_S
                {
                    break; // lo que sonó con esta trama aún puede llegar
                }
                far.read_at(far_from, &mut self.render)
            };
            if self.mic_moved.is_some_and(|m| at + FRAME as i64 > m) {
                self.mic_moved = None;
                moved = true;
            }
            self.last_at = at;
            self.take_frame();
            self.step(moved);
            self.emit(&mut done);
        }
        done
    }

    /// La siguiente trama del micrófono (con ceros si se acaba, solo al vaciar).
    fn take_frame(&mut self) {
        let mut n = 0;
        while n < FRAME {
            let Some(c) = self.chunks.get(self.next.0) else {
                break;
            };
            let take = (c.raw.len() - self.next.1).min(FRAME - n);
            self.frame[n..n + take].copy_from_slice(&c.raw[self.next.1..self.next.1 + take]);
            n += take;
            self.next.1 += take;
            if self.next.1 == c.raw.len() {
                self.next = (self.next.0 + 1, 0);
            }
        }
        self.pending -= n;
        self.frame[n..].fill(0.0);
    }

    /// Pasa la trama por el AEC (`moved`: lo que suena con ella se recolocó); si falla, se avisa una
    /// vez y en adelante pasa sin limpiar.
    fn step(&mut self, moved: bool) {
        let ok = match self.aec.as_mut() {
            None => false,
            Some(aec) => {
                let (frame, render, out) = (&self.frame, &self.render, &mut self.out);
                match catch_unwind(AssertUnwindSafe(|| {
                    if moved {
                        aec.echo_path_changed();
                    }
                    aec.render(render)?;
                    aec.capture(frame, out)
                })) {
                    Ok(Ok(())) => true,
                    Ok(Err(e)) => {
                        log::error!(
                            "La cancelación de eco falló ({e}); el micrófono sigue sin ella"
                        );
                        false
                    }
                    Err(_) => {
                        log::error!("La cancelación de eco falló; el micrófono sigue sin ella");
                        false
                    }
                }
            }
        };
        let lat = self.latency;
        if !ok {
            self.aec = None;
            // con el mismo retardo: lo que el AEC tenía dentro se sustituye por el crudo
            self.out[..lat].copy_from_slice(&self.tail);
            self.out[lat..].copy_from_slice(&self.frame[..FRAME - lat]);
        }
        self.tail.copy_from_slice(&self.frame[FRAME - lat..]);
    }

    /// Reparte la trama limpia entre sus trozos y devuelve los que ya están completos.
    fn emit(&mut self, done: &mut Vec<(f64, Vec<f32>)>) {
        let skip = self.skip.min(FRAME);
        self.skip -= skip;
        let mut src = &self.out[skip..];
        for c in self.chunks.iter_mut() {
            if src.is_empty() {
                break;
            }
            let take = (c.raw.len() - c.clean.len()).min(src.len());
            c.clean.extend_from_slice(&src[..take]);
            src = &src[take..];
        }
        while self
            .chunks
            .front()
            .is_some_and(|c| c.clean.len() == c.raw.len())
        {
            if let Some(c) = self.chunks.pop_front() {
                done.push((c.t, c.clean));
                self.next.0 = self.next.0.saturating_sub(1);
            }
        }
    }
}

/// Deja pasar el micrófono tal cual (si no se pudo crear el AEC3).
struct Bypass;

impl EchoProcessor for Bypass {
    fn render(&mut self, _frame: &[f32]) -> Result<(), String> {
        Ok(())
    }

    fn capture(&mut self, frame: &[f32], out: &mut [f32]) -> Result<(), String> {
        out.copy_from_slice(frame);
        Ok(())
    }

    fn latency(&self) -> usize {
        0
    }
}
