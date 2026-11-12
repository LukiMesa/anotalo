//! Lógica de Anotalo sin hardware: audio a 16 kHz, detección de voz, frases, filtro,
//! transcripción a Markdown, modelos y la sesión de grabación.
pub mod filter;
pub mod resample;
pub mod segmenter;
pub mod vad;
