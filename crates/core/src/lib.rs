//! Lógica de Anotalo sin hardware: audio a 16 kHz, detección de voz, frases, filtro,
//! transcripción a Markdown, modelos y la sesión de grabación.
pub mod engine;
pub mod filter;
pub mod models;
pub mod resample;
pub mod routing;
pub mod segmenter;
pub mod source;
pub mod transcript;
pub mod vad;
