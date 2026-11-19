//! Hasta que se oye a otra persona en la llamada, lo que dices espera (en memoria: ni al archivo ni
//! a la ventana). Las apps de llamadas abren el micrófono en la sala de espera y lo mantienen
//! abierto aunque te silencies, así que lo que hablas antes de que entre nadie (con alguien a tu
//! lado, por teléfono…) se transcribiría. Cuando habla otra persona se queda lo tuyo de los últimos
//! `LOOKBACK_S` (el saludo) y lo anterior se descarta. Si no habla nadie más en toda la llamada
//! (quizá falló el audio de los demás) se guarda todo, como antes.

/// Lo tuyo que se guarda de antes de que hable otra persona.
pub const LOOKBACK_S: f64 = 30.0;

/// Una frase transcrita, tal como irá al archivo.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub speaker: String,
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    /// El segmento de voz de "Otros" del que sale (para la etiqueta final de la persona).
    pub voice: Option<u32>,
}

#[derive(Debug, Default)]
pub struct StartGate {
    /// Cuándo empezó a hablar la primera otra persona.
    opened_at: Option<f64>,
    held: Vec<Line>,
}

impl StartGate {
    pub fn new() -> StartGate {
        StartGate::default()
    }

    pub fn is_open(&self) -> bool {
        self.opened_at.is_some()
    }

    fn recent(opened_at: f64, l: &Line) -> bool {
        l.end_s >= opened_at - LOOKBACK_S
    }

    /// Una frase tuya: sale si ya habló alguien (y no es de antes de `LOOKBACK_S`); si no, espera.
    pub fn mine(&mut self, l: Line) -> Vec<Line> {
        match self.opened_at {
            Some(at) if Self::recent(at, &l) => vec![l],
            Some(_) => {
                log::debug!("Se descarta una frase tuya de antes de que hablara nadie");
                Vec::new()
            }
            None => {
                self.held.push(l);
                Vec::new()
            }
        }
    }

    /// Una frase de otra persona: la primera suelta las tuyas recientes (en orden) y descarta las
    /// anteriores.
    pub fn other(&mut self, l: Line) -> Vec<Line> {
        if self.opened_at.is_some() {
            return vec![l];
        }
        let at = l.start_s;
        self.opened_at = Some(at);
        let held = std::mem::take(&mut self.held);
        let before = held.len();
        let mut out: Vec<Line> = held.into_iter().filter(|h| Self::recent(at, h)).collect();
        if out.len() < before {
            log::info!(
                "Descartadas {} frases tuyas de antes de que hablara nadie más",
                before - out.len()
            );
        }
        out.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        out.push(l);
        out
    }

    /// Al colgar: si no habló nadie más, todo lo tuyo (en orden).
    pub fn finish(&mut self) -> Vec<Line> {
        let mut out = std::mem::take(&mut self.held);
        out.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        out
    }
}
