//! Huellas de voz con NVIDIA NeMo TitaNet-small (ONNX) en tract (spec §2.4.5). La longitud de la
//! entrada es simbólica: con una fija, el optimizador de tract falla en un EinSum.
//!
//! Las huellas se centran restando una media de fondo (`assets/titanet_small_mu.bin`, la media de
//! las huellas normalizadas de las llamadas de ajuste del spike, con audio degradado): así separan
//! mejor a las personas en audio de llamada.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use tract_onnx::prelude::*;

use super::fbank::{N_MELS, fbank, normalize_per_feature};

pub const DIM: usize = 192;
/// Segmentos más cortos no se embeben (no dan una huella fiable).
pub const MIN_EMBED_S: f64 = 0.3;
const MIN_SAMPLES: usize = (MIN_EMBED_S * 16_000.0) as usize;

const MU_BYTES: &[u8] = include_bytes!("../../assets/titanet_small_mu.bin");

/// Lo que la sesión necesita para separar voces (el modelo real o uno falso en los tests).
pub trait Embed: Send + Sync {
    /// Huella centrada y de norma 1 de un segmento de "Otros", o None si es demasiado corto o el
    /// modelo falló.
    fn embed(&self, audio: &[f32]) -> Option<Vec<f32>>;
}

type Plan = Arc<TypedRunnableModel>;

pub struct Embedder {
    plan: Plan,
}

impl Embedder {
    pub fn load(path: &Path) -> Result<Embedder, String> {
        Self::plan(path)
            .map(|plan| Embedder { plan })
            .map_err(|e| format!("no se pudo cargar el modelo de voces: {e}"))
    }

    fn plan(path: &Path) -> TractResult<Plan> {
        let mut model = tract_onnx::onnx().model_for_path(path)?;
        let t: TDim = model.symbols.sym("T").into();
        model.set_input_fact(
            0,
            f32::fact([TDim::from(1), TDim::from(N_MELS as i64), t]).into(),
        )?;
        model.set_input_fact(1, i64::fact([1]).into())?;
        model.into_optimized()?.into_runnable()
    }

    /// La huella tal cual sale del modelo (192 valores), sin centrar.
    pub fn embed_raw(&self, audio: &[f32]) -> Result<Vec<f32>, String> {
        let mut feats = fbank(audio);
        if feats.is_empty() {
            return Err("audio demasiado corto".into());
        }
        normalize_per_feature(&mut feats);
        let t = feats.len();
        let mut x = vec![0f32; N_MELS * t];
        for (i, row) in feats.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                x[j * t + i] = *v;
            }
        }
        let run = || -> TractResult<Vec<f32>> {
            let x = tract_ndarray::Array3::from_shape_vec((1, N_MELS, t), x)?;
            let len = tract_ndarray::Array1::from_vec(vec![t as i64]);
            let out = self
                .plan
                .run(tvec!(Tensor::from(x).into(), Tensor::from(len).into()))?;
            Ok(out[1]
                .to_plain_array_view::<f32>()?
                .iter()
                .copied()
                .collect())
        };
        let v = run().map_err(|e| format!("el modelo de voces falló: {e}"))?;
        if v.len() != DIM {
            return Err(format!("huella de {} valores, se esperaban {DIM}", v.len()));
        }
        Ok(v)
    }
}

impl Embed for Embedder {
    fn embed(&self, audio: &[f32]) -> Option<Vec<f32>> {
        if audio.len() < MIN_SAMPLES {
            return None;
        }
        match self.embed_raw(audio).and_then(|raw| center_checked(&raw)) {
            Ok(v) => Some(v),
            Err(e) => {
                log::debug!("Sin huella de voz para un segmento: {e}");
                None
            }
        }
    }
}

fn mu() -> &'static [f32] {
    static MU: OnceLock<Vec<f32>> = OnceLock::new();
    MU.get_or_init(|| {
        MU_BYTES
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    })
}

/// `v / |v|` (cero si es nulo).
pub fn l2n(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt() + 1e-12;
    v.iter().map(|x| x / n).collect()
}

/// Huella centrada: `l2n(l2n(raw) − MU)`.
pub fn center(raw: &[f32]) -> Vec<f32> {
    let d: Vec<f32> = l2n(raw).iter().zip(mu()).map(|(a, m)| a - m).collect();
    l2n(&d)
}

/// Como `center`, pero rechaza la huella si algún valor no es finito: un NaN se guardaría como
/// `null` y dejaría ilegible el archivo de voces de la llamada.
pub fn center_checked(raw: &[f32]) -> Result<Vec<f32>, String> {
    let v = center(raw);
    if v.iter().all(|x| x.is_finite()) {
        Ok(v)
    } else {
        Err("la huella tiene valores no finitos".into())
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}
