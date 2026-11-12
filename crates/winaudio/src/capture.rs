//! Captura WASAPI en modo compartido y por eventos, convertida por Windows a 48 kHz estéreo i16 y
//! remuestreada a 16 kHz mono (como la v0.6.3):
//!
//! - [`MicSource`]: un micrófono.
//! - [`EndpointLoopbackSource`]: todo lo que suena en UNA salida concreta (la que usa la llamada,
//!   p. ej. los auriculares Bluetooth en manos libres).
//! - [`ProcessLoopbackSource`]: el audio de UN programa y sus hijos (Windows 10 2004+), sea cual sea
//!   la salida; sin notificaciones ni otras apps.
//!
//! Cada una corre en su propio hilo con COM multihilo. Si el dispositivo desaparece (auriculares
//! apagados) o el programa se cierra, el hilo termina e `is_active()` pasa a false: la sesión la
//! reabre.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use anotalo_core::resample::{Resampler, TARGET_RATE};
use anotalo_core::source::{AudioSource, OnAudio, now_s};
use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync, EDataFlow,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVE_FORMAT_PCM, WAVEFORMATEX, eCapture, eConsole,
    eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{BLOB, CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Error, HRESULT, IUnknown, Interface, PCWSTR, Ref, Result, implement};

use crate::coreaudio::in_mta;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// Búfer de WASAPI: 200 ms.
const BUFFER_HNS: i64 = 2_000_000;

#[derive(Debug, Clone)]
enum Target {
    /// Un micrófono (None = el predeterminado al arrancar).
    Mic(Option<String>),
    /// Lo que suena en una salida (None = la predeterminada al arrancar).
    Output(Option<String>),
    /// El audio de un programa y sus hijos.
    Process(u32),
}

/// Un micrófono.
pub struct MicSource(Capture);

impl MicSource {
    /// `None`: el micrófono predeterminado en el momento de arrancar.
    pub fn new(endpoint_id: Option<String>) -> MicSource {
        MicSource(Capture::new(
            Target::Mic(endpoint_id),
            "el micrófono".into(),
        ))
    }
}

/// Todo lo que suena en una salida concreta.
pub struct EndpointLoopbackSource(Capture);

impl EndpointLoopbackSource {
    /// `None`: la salida predeterminada en el momento de arrancar.
    pub fn new(endpoint_id: Option<String>, name: String) -> EndpointLoopbackSource {
        let shown = if name.is_empty() {
            endpoint_id
                .clone()
                .unwrap_or_else(|| "la salida predeterminada".into())
        } else {
            name
        };
        EndpointLoopbackSource(Capture::new(
            Target::Output(endpoint_id),
            format!("lo que suena en {shown}"),
        ))
    }
}

/// El audio de un programa y de sus procesos hijos.
pub struct ProcessLoopbackSource(Capture);

impl ProcessLoopbackSource {
    pub fn new(pid: u32) -> ProcessLoopbackSource {
        ProcessLoopbackSource(Capture::new(
            Target::Process(pid),
            format!("el audio del proceso {pid}"),
        ))
    }

    /// Comprueba que Windows deja capturar el audio de ese programa (si no, se usa su salida).
    pub fn probe_pid(pid: u32) -> std::result::Result<(), String> {
        in_mta(Duration::from_secs(10), move || {
            // SAFETY: en un hilo con COM iniciado.
            unsafe {
                process_client(pid).and_then(|client| {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK
                            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                        BUFFER_HNS,
                        0,
                        &format(),
                        None,
                    )
                })
            }
            .map_err(|e| e.to_string())
        })
    }
}

macro_rules! audio_source {
    ($($t:ty),*) => {$(
        impl AudioSource for $t {
            fn name(&self) -> &str {
                &self.0.name
            }
            fn start(&mut self, on_audio: OnAudio) -> std::result::Result<(), String> {
                self.0.start(on_audio)
            }
            fn stop(&mut self) {
                self.0.stop()
            }
            fn is_active(&self) -> bool {
                self.0.is_active()
            }
        }
    )*};
}

audio_source!(MicSource, EndpointLoopbackSource, ProcessLoopbackSource);

struct Worker {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

struct Capture {
    target: Target,
    name: String,
    worker: Option<Worker>,
}

impl Capture {
    fn new(target: Target, name: String) -> Capture {
        Capture {
            target,
            name,
            worker: None,
        }
    }

    fn start(&mut self, on_audio: OnAudio) -> std::result::Result<(), String> {
        self.stop();
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (target, name) = (self.target.clone(), self.name.clone());
        let (stop2, alive2) = (stop.clone(), alive.clone());
        let thread = std::thread::Builder::new()
            .name("captura".into())
            .spawn(move || {
                run(&target, &name, on_audio, &stop2, &ready_tx);
                alive2.store(false, Ordering::SeqCst);
            })
            .map_err(|e| format!("no se pudo arrancar la captura: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {
                self.worker = Some(Worker {
                    stop,
                    alive,
                    thread,
                });
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                // colgado al abrir: se le pide parar y se abandona (no se espera)
                stop.store(true, Ordering::SeqCst);
                Err(format!("{}: la captura no arrancó", self.name))
            }
        }
    }

    fn stop(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop.store(true, Ordering::SeqCst);
            if w.thread.join().is_err() {
                log::error!("La captura de {} terminó con un pánico", self.name);
            }
        }
    }

    fn is_active(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|w| w.alive.load(Ordering::SeqCst))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    target: &Target,
    name: &str,
    mut on_audio: OnAudio,
    stop: &AtomicBool,
    ready: &mpsc::Sender<std::result::Result<(), String>>,
) {
    let _com = match crate::coreaudio::Com::init_mta() {
        Ok(com) => com,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: en un hilo con COM iniciado; el stream vive en este hilo.
    let mut stream = match unsafe { Stream::open(target) } {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(format!("no se pudo capturar {name}: {e}")));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    log::info!("Capturando {name}");
    // SAFETY: ver arriba.
    if let Err(e) = unsafe { stream.pump(&mut on_audio, stop) } {
        log::warn!("Captura de {name} interrumpida: {e}");
    }
}

/// Un HANDLE que se cierra solo.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: el handle es nuestro y se cierra una vez.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Handle,
    /// Para process loopback: se señala cuando el programa termina.
    process: Option<Handle>,
    resampler: Resampler,
}

impl Stream {
    unsafe fn open(target: &Target) -> Result<Stream> {
        // SAFETY: llamadas COM en un hilo con COM iniciado.
        unsafe {
            let (client, loopback, process) = match target {
                Target::Mic(id) => (endpoint_client(id.as_deref(), eCapture)?, 0, None),
                Target::Output(id) => (
                    endpoint_client(id.as_deref(), eRender)?,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    None,
                ),
                Target::Process(pid) => {
                    let client = process_client(*pid)?;
                    let process = Handle(OpenProcess(PROCESS_SYNCHRONIZE, false, *pid)?);
                    (client, AUDCLNT_STREAMFLAGS_LOOPBACK, Some(process))
                }
            };
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                loopback | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                BUFFER_HNS,
                0,
                &format(),
                None,
            )?;
            let event = Handle(CreateEventW(None, false, false, PCWSTR::null())?);
            client.SetEventHandle(event.0)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Stream {
                client,
                capture,
                event,
                process,
                resampler: Resampler::new(RATE, CHANNELS),
            })
        }
    }

    /// Lee y entrega audio hasta que se pida parar, el programa termine o el dispositivo falle.
    unsafe fn pump(&mut self, on_audio: &mut OnAudio, stop: &AtomicBool) -> Result<()> {
        let mut pcm: Vec<i16> = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            // SAFETY: handles e interfaces vivos mientras viva `self`; el búfer de WASAPI es válido
            // entre GetBuffer y ReleaseBuffer y tiene `n` tramas de 2 canales i16.
            unsafe {
                WaitForSingleObject(self.event.0, 100);
                if let Some(p) = &self.process
                    && WaitForSingleObject(p.0, 0) == WAIT_OBJECT_0
                {
                    log::info!("El programa terminó: fin de su captura");
                    return Ok(());
                }
                pcm.clear();
                while self.capture.GetNextPacketSize()? > 0 {
                    let mut data = std::ptr::null_mut();
                    let mut n = 0u32;
                    let mut flags = 0u32;
                    self.capture
                        .GetBuffer(&mut data, &mut n, &mut flags, None, None)?;
                    let samples = n as usize * usize::from(CHANNELS);
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        pcm.resize(pcm.len() + samples, 0);
                    } else {
                        pcm.extend_from_slice(std::slice::from_raw_parts(
                            data as *const i16,
                            samples,
                        ));
                    }
                    self.capture.ReleaseBuffer(n)?;
                }
            }
            if !pcm.is_empty() {
                let out = self.resampler.push_i16(&pcm);
                deliver(on_audio, &out);
            }
        }
        let tail = self.resampler.finish();
        deliver(on_audio, &tail);
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: el cliente sigue vivo; parar dos veces no hace daño.
        let _ = unsafe { self.client.Stop() };
    }
}

fn deliver(on_audio: &mut OnAudio, x: &[f32]) {
    if !x.is_empty() {
        on_audio(now_s() - x.len() as f64 / f64::from(TARGET_RATE), x);
    }
}

fn format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(CHANNELS) * 2,
        nBlockAlign: CHANNELS * 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

unsafe fn endpoint_client(id: Option<&str>, flow: EDataFlow) -> Result<IAudioClient> {
    // SAFETY: en un hilo con COM iniciado; `wide` vive durante GetDevice.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = match id {
            Some(id) => {
                let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
                enumerator.GetDevice(PCWSTR(wide.as_ptr()))?
            }
            None => enumerator.GetDefaultAudioEndpoint(flow, eConsole)?,
        };
        device.Activate(CLSCTX_ALL, None)
    }
}

/// Avisa cuando Windows termina de activar la captura del programa.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(mpsc::SyncSender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> Result<()> {
        let _ = self.0.try_send(());
        Ok(())
    }
}

unsafe fn process_client(pid: u32) -> Result<IAudioClient> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // VT_BLOB que apunta a `params`, en esta pila. Nunca se suelta: el Drop de PROPVARIANT llama a
    // PropVariantClear, que liberaría `params` como si fuera memoria de COM (corrompe el heap).
    let pv = std::mem::ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw mut params).cast(),
                    },
                },
            }),
        },
    });
    let (tx, rx) = mpsc::sync_channel(1);
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(tx).into();
    // SAFETY: `pv` y `params` siguen vivos hasta que la activación termina (se espera abajo).
    unsafe {
        let op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*pv),
            &handler,
        )?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| Error::new(E_FAIL, "ActivateAudioInterfaceAsync no respondió"))?;
        let mut hr = HRESULT(0);
        let mut unknown: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unknown)?;
        hr.ok()?;
        unknown
            .ok_or_else(|| Error::new(E_FAIL, "la activación no devolvió nada"))?
            .cast()
    }
}
