//! Lógica de Anotalo sin hardware: audio a 16 kHz, detección de voz, frases, filtro,
//! transcripción a Markdown, modelos y la sesión de grabación.
pub mod app;
pub mod applog;
pub mod config;
pub mod detector;
pub mod engine;
pub mod filter;
pub mod hotkey;
pub mod library;
pub mod meta;
pub mod model;
pub mod models;
pub mod notify;
pub mod paths;
pub mod resample;
pub mod routing;
pub mod segmenter;
pub mod session;
pub mod source;
pub mod speaker;
pub mod transcript;
pub mod vad;
