//! Hasta que se oye a otra persona en la llamada, lo que dices espera: si nadie más habla en
//! `LOOKBACK_S`, se descarta (estabas solo en la sala de espera, o hablando con alguien a tu lado).
use anotalo_core::gate::{LOOKBACK_S, Line, StartGate};

fn mine(start_s: f64) -> Line {
    Line {
        speaker: "Yo".into(),
        start_s,
        end_s: start_s + 2.0,
        text: format!("yo en {start_s}"),
        voice: None,
    }
}

fn other(start_s: f64) -> Line {
    Line {
        speaker: "Otros".into(),
        start_s,
        end_s: start_s + 2.0,
        text: format!("otro en {start_s}"),
        voice: Some(7),
    }
}

fn starts(lines: &[Line]) -> Vec<(String, f64)> {
    lines
        .iter()
        .map(|l| (l.speaker.clone(), l.start_s))
        .collect()
}

#[test]
fn test_lookback_is_thirty_seconds() {
    assert_eq!(LOOKBACK_S, 30.0);
}

#[test]
fn test_my_lines_wait_until_someone_else_speaks() {
    let mut g = StartGate::new();
    assert!(g.mine(mine(100.0)).is_empty());
    assert!(!g.is_open());
}

#[test]
fn test_first_other_line_releases_my_recent_lines_and_drops_older_ones() {
    let mut g = StartGate::new();
    for t in [10.0, 60.0, 75.0] {
        assert!(g.mine(mine(t)).is_empty());
    }
    // la otra persona empieza en 90: se queda lo que termina desde 60 (90 - 30)
    let out = g.other(other(90.0));
    assert_eq!(
        starts(&out),
        vec![
            ("Yo".into(), 60.0),
            ("Yo".into(), 75.0),
            ("Otros".into(), 90.0)
        ]
    );
    assert_eq!(out[2].voice, Some(7));
    assert!(g.is_open());
}

#[test]
fn test_released_lines_come_out_in_order() {
    let mut g = StartGate::new();
    g.mine(mine(80.0));
    g.mine(mine(70.0));
    assert_eq!(
        starts(&g.other(other(85.0))),
        vec![
            ("Yo".into(), 70.0),
            ("Yo".into(), 80.0),
            ("Otros".into(), 85.0)
        ]
    );
}

#[test]
fn test_once_open_everything_passes() {
    let mut g = StartGate::new();
    g.other(other(5.0));
    assert_eq!(starts(&g.mine(mine(6.0))), vec![("Yo".into(), 6.0)]);
    assert_eq!(starts(&g.other(other(9.0))), vec![("Otros".into(), 9.0)]);
    assert!(g.finish().is_empty());
}

#[test]
fn test_late_line_from_before_the_lookback_is_dropped() {
    let mut g = StartGate::new();
    g.other(other(200.0));
    // tardó en transcribirse, pero es de cuando aún no había nadie
    assert!(g.mine(mine(100.0)).is_empty());
    assert_eq!(starts(&g.mine(mine(180.0))), vec![("Yo".into(), 180.0)]);
}

#[test]
fn test_if_nobody_else_ever_speaks_everything_is_kept() {
    let mut g = StartGate::new();
    g.mine(mine(300.0));
    g.mine(mine(1.0));
    assert_eq!(
        starts(&g.finish()),
        vec![("Yo".into(), 1.0), ("Yo".into(), 300.0)]
    );
    assert!(g.finish().is_empty());
}
