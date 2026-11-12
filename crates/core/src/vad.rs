//! Silero VAD v6 en streaming (port de calltx/vad.py): el mismo modelo que la app de Python,
//! ejecutado con tract (Rust puro) y embebido en el exe.

use tract_onnx::prelude::*;

/// Muestras nuevas por trama (32 ms a 16 kHz).
pub const FRAME: usize = 512;
/// Muestras de la trama anterior que el modelo recibe delante de la nueva.
pub const CONTEXT: usize = 64;
const STATE: usize = 128;

/// El modelo de faster-whisper 1.2.1 (silero_vad_v6.onnx, MIT; ver assets/SILERO-LICENSE.txt).
pub(crate) const MODEL: &[u8] = include_bytes!("../assets/silero_vad_v6.onnx");

type Plan = std::sync::Arc<TypedRunnableModel>;

/// Detector de voz con estado: probabilidad de voz de cada trama de 512 muestras a 16 kHz.
pub struct Vad {
    plan: Plan,
    h: Tensor,
    c: Tensor,
    context: [f32; CONTEXT],
}

impl Vad {
    pub fn new() -> Result<Vad, String> {
        let plan =
            Self::load().map_err(|e| format!("no se pudo cargar el detector de voz: {e}"))?;
        let mut vad = Vad {
            plan,
            h: Self::zero_state(),
            c: Self::zero_state(),
            context: [0.0; CONTEXT],
        };
        vad.reset();
        Ok(vad)
    }

    fn load() -> TractResult<Plan> {
        tract_onnx::onnx()
            .model_for_read(&mut std::io::Cursor::new(MODEL))?
            .with_input_fact(0, f32::fact([1, CONTEXT + FRAME]).into())?
            .with_input_fact(1, f32::fact([1, 1, STATE]).into())?
            .with_input_fact(2, f32::fact([1, 1, STATE]).into())?
            .into_optimized()?
            .into_runnable()
    }

    fn zero_state() -> Tensor {
        Tensor::zero::<f32>(&[1, 1, STATE]).expect("estado del detector de voz")
    }

    /// Vuelve al estado inicial (como al crear el detector).
    pub fn reset(&mut self) {
        self.h = Self::zero_state();
        self.c = Self::zero_state();
        self.context = [0.0; CONTEXT];
    }

    /// Probabilidad de voz de una trama; guarda el estado y el contexto para la siguiente.
    pub fn prob(&mut self, frame: &[f32; FRAME]) -> f32 {
        let mut input = Vec::with_capacity(CONTEXT + FRAME);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(frame);
        let input =
            Tensor::from_shape(&[1, CONTEXT + FRAME], &input).expect("entrada del detector de voz");
        let out = self
            .plan
            .run(tvec!(
                input.into_tvalue(),
                self.h.clone().into_tvalue(),
                self.c.clone().into_tvalue()
            ))
            .expect("el detector de voz falló");
        let p = out[0]
            .try_as_plain_ram()
            .ok()
            .and_then(|v| v.as_slice::<f32>().ok().and_then(|s| s.first().copied()))
            .unwrap_or(0.0);
        self.h = out[1].clone().into_tensor();
        self.c = out[2].clone().into_tensor();
        self.context.copy_from_slice(&frame[FRAME - CONTEXT..]);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    fn fixture() -> (String, Vec<f32>) {
        let text = std::fs::read_to_string(root().join("crates/core/tests/fixtures/vad_jfk.json"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let probs = v["probs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_f64().unwrap() as f32)
            .collect();
        (v["sha256"].as_str().unwrap().to_string(), probs)
    }

    fn jfk_frames() -> Vec<[f32; FRAME]> {
        let mut r =
            hound::WavReader::open(root().join("third_party/whisper.cpp/samples/jfk.wav")).unwrap();
        let x: Vec<f32> = r
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect();
        x.as_chunks::<FRAME>().0.to_vec()
    }

    #[test]
    fn matches_python_on_jfk() {
        let (_, expected) = fixture();
        let mut vad = Vad::new().unwrap();
        let frames = jfk_frames();
        assert_eq!(frames.len(), expected.len());
        for (i, (f, want)) in frames.iter().zip(&expected).enumerate() {
            let got = vad.prob(f);
            assert!((got - want).abs() <= 1e-3, "trama {i}: {got} vs {want}");
        }
    }

    #[test]
    fn speech_and_silence() {
        let mut vad = Vad::new().unwrap();
        let max = jfk_frames()
            .iter()
            .map(|f| vad.prob(f))
            .fold(0.0f32, f32::max);
        assert!(max > 0.9, "{max}");
        vad.reset();
        let quiet = (0..31)
            .map(|_| vad.prob(&[0.0; FRAME]))
            .fold(0.0f32, f32::max);
        assert!(quiet < 0.1, "{quiet}");
    }

    #[test]
    fn reset_restores_initial_state() {
        let frames = jfk_frames();
        let mut vad = Vad::new().unwrap();
        let a: Vec<f32> = frames[..40].iter().map(|f| vad.prob(f)).collect();
        vad.reset();
        let b: Vec<f32> = frames[..40].iter().map(|f| vad.prob(f)).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn embedded_model_is_the_pinned_one() {
        use sha2::{Digest, Sha256};
        let (sha, _) = fixture();
        let got: String = Sha256::digest(MODEL)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(got, sha);
    }
}
