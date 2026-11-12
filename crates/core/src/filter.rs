//! Frases que Whisper inventa sobre ruido o silencio (port de `_normalize` e `is_hallucination`
//! de calltx/asr.py). Vienen sobre todo de subtítulos de YouTube.

use std::sync::LazyLock;

use regex::Regex;

const BLOCKLIST: &[&str] = &[
    "gracias por ver",
    "gracias por ver el video",
    "gracias por ver el vídeo",
    "subtítulos realizados por la comunidad de amaraorg",
    "subtitulos realizados por la comunidad de amaraorg",
    "suscríbete",
    "suscribete",
    "gracias",
    "música",
    "musica",
];

static NOT_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\s]").unwrap());

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\bsuscr[ií]bete\b",
        r"\bamara ?org\b",
        r"^gracias por (ver|vernos|su atención)",
        r"\bdale (a )?like\b",
        r"^subt[ií]tulos (realizados )?por\b",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

/// Minúsculas, sin signos (se conservan letras, números, tildes y ñ) y espacios simples.
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    NOT_WORD
        .replace_all(&lower, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// ¿Es una frase inventada? Vacía, de muy poca confianza sobre probable silencio, o típica de
/// subtítulos de YouTube.
pub fn is_hallucination(text: &str, avg_logprob: f32, no_speech_prob: f32) -> bool {
    let norm = normalize(text);
    if norm.is_empty() {
        return true;
    }
    if no_speech_prob > 0.6 && avg_logprob < -1.0 {
        return true;
    }
    if BLOCKLIST.contains(&norm.as_str()) {
        return true;
    }
    PATTERNS.iter().any(|p| p.is_match(&norm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hallucination_rules() {
        assert!(is_hallucination("", -0.1, 0.0));
        assert!(is_hallucination("...", -0.1, 0.0));
        assert!(is_hallucination(
            "Subtítulos realizados por la comunidad de Amara.org",
            -0.1,
            0.0
        ));
        assert!(!is_hallucination("Quiero cambiar el logo", -0.3, 0.1));
    }

    #[test]
    fn test_youtube_style_hallucinations_filtered() {
        for t in [
            "¡Suscríbete al canal!",
            "Suscríbete y dale like",
            "Gracias por ver el vídeo.",
            "Subtítulos por la comunidad de Amara.org",
            "¡Gracias por vernos!",
        ] {
            assert!(is_hallucination(t, -0.3, 0.1), "{t}");
        }
        assert!(!is_hallucination(
            "Me suscribí al plan premium la semana pasada",
            -0.3,
            0.1
        ));
    }

    #[test]
    fn keeps_accents_and_enye() {
        assert_eq!(normalize("¿Qué tal, Señor?"), "qué tal señor");
        // poca confianza y probable silencio: se descarta aunque el texto sea normal
        assert!(is_hallucination("Hola", -1.2, 0.7));
        assert!(!is_hallucination("Hola", -1.2, 0.5));
    }
}
