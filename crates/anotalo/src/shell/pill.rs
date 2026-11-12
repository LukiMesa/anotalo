//! La píldora del indicador (port de `render_pill` de calltx/ui/indicator.py): fondo redondeado,
//! punto rojo y barras de nivel mientras se graba, un giro mientras se transcribe y una marca de
//! verificación al terminar. Se dibuja con tiny-skia (con antialias, sin supermuestreo) y el texto
//! con fontdue (JetBrains Mono, incluida en el exe).

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PremultipliedColorU8,
    Stroke, Transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillState {
    Recording,
    Transcribing,
    Copied,
    Saved,
    Marked,
}

pub const ENTER_S: f64 = 0.18;
pub const EXIT_S: f64 = 0.12;
pub const RISE_PX: i32 = 8;

// Tokens del tema Noche (calltx/ui/web/tokens.css), como en Python.
const BG: (u8, u8, u8, u8) = (16, 20, 31, 240);
const BORDER: (u8, u8, u8, u8) = (46, 56, 80, 255);
const INK: (u8, u8, u8, u8) = (238, 243, 247, 255);
const ORBITA: (u8, u8, u8, u8) = (46, 197, 230, 255);
const ERROR: (u8, u8, u8, u8) = (255, 122, 122, 255);
const ERROR_HALO: (u8, u8, u8, u8) = (255, 122, 122, 50);
const EXITO: (u8, u8, u8, u8) = (61, 220, 151, 255);
const LINE_STRONG: (u8, u8, u8, u8) = (90, 104, 131, 255);

fn font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../../../../calltx/ui/web/fonts/JetBrainsMono.ttf");
        Font::from_bytes(bytes, FontSettings::default()).expect("JetBrains Mono incluida")
    })
}

fn text_for(state: PillState, elapsed_s: f64) -> String {
    match state {
        PillState::Recording => fmt_clock(elapsed_s),
        PillState::Transcribing => "Transcribiendo…".into(),
        PillState::Copied => "Copiado al portapapeles".into(),
        PillState::Saved => "Transcripción guardada".into(),
        PillState::Marked => "Marcado".into(),
    }
}

fn paint(c: (u8, u8, u8, u8)) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c.0, c.1, c.2, c.3));
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// Rectángulo redondeado (curvas de Bézier que aproximan los cuartos de círculo).
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let c = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.close();
    pb.finish()
}

fn text_width(text: &str, px: f32) -> f32 {
    text.chars()
        .map(|ch| font().metrics(ch, px).advance_width)
        .sum()
}

/// Escribe el texto con su línea base centrada en `cy`, mezclando la cobertura de cada glifo.
fn draw_text(pm: &mut Pixmap, text: &str, x: f32, cy: f32, px: f32, c: (u8, u8, u8, u8)) {
    let f = font();
    let (ascent, descent) = f
        .horizontal_line_metrics(px)
        .map_or((px * 0.8, -px * 0.2), |m| (m.ascent, m.descent));
    let baseline = cy + (ascent + descent) / 2.0;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let mut pen = x;
    for ch in text.chars() {
        let (m, cov) = f.rasterize(ch, px);
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (baseline - (m.height as f32 + m.ymin as f32)).round() as i32;
        let pixels = pm.pixels_mut();
        for row in 0..m.height as i32 {
            for col in 0..m.width as i32 {
                let (px_x, px_y) = (gx + col, gy + row);
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    continue;
                }
                let a =
                    u32::from(cov[(row * m.width as i32 + col) as usize]) * u32::from(c.3) / 255;
                if a == 0 {
                    continue;
                }
                let i = (px_y * w + px_x) as usize;
                let d = pixels[i];
                let blend = |src: u8, dst: u8| -> u8 {
                    ((u32::from(src) * a + u32::from(dst) * (255 - a)) / 255) as u8
                };
                let (r, g, b) = (c.0, c.1, c.2);
                let out_a = (a + u32::from(d.alpha()) * (255 - a) / 255) as u8;
                let px = PremultipliedColorU8::from_rgba(
                    blend(r, d.red()).min(out_a),
                    blend(g, d.green()).min(out_a),
                    blend(b, d.blue()).min(out_a),
                    out_a,
                );
                if let Some(px) = px {
                    pixels[i] = px;
                }
            }
        }
        pen += m.advance_width;
    }
}

pub fn render_pill(
    state: PillState,
    levels: &[f32],
    elapsed_s: f64,
    scale: f32,
    spin: f32,
) -> Pixmap {
    let k = scale.max(0.5);
    let r = |v: f32| (v * k).round();
    let h = r(32.0);
    let (pad_l, pad_r, gap) = (r(12.0), r(14.0), r(9.0));
    let px = (11.5 * k).max(8.0);
    let text = text_for(state, elapsed_s);
    let tw = text_width(&text, px).round();
    let recording = state == PillState::Recording;
    let icon_w = if recording { r(8.0) } else { r(14.0) };
    let bars_w = if recording { r(28.0) } else { 0.0 };
    let w = pad_l + icon_w + gap + if recording { bars_w + gap } else { 0.0 } + tw + pad_r;
    let mut pm = Pixmap::new(w as u32, h as u32).expect("tamaño de la píldora");
    let t = Transform::identity();

    let bw = k.round().max(1.0);
    if let Some(bg) = rounded_rect(bw / 2.0, bw / 2.0, w - bw, h - bw, (h - bw) / 2.0) {
        pm.fill_path(&bg, &paint(BG), FillRule::Winding, t, None);
        pm.stroke_path(&bg, &paint(BORDER), &stroke(bw), t, None);
    }
    let cy = h / 2.0;
    let mut x = pad_l;
    match state {
        PillState::Recording => {
            let cx = x + 4.0 * k;
            if let Some(halo) = PathBuilder::from_circle(cx, cy, 7.0 * k) {
                pm.fill_path(&halo, &paint(ERROR_HALO), FillRule::Winding, t, None);
            }
            if let Some(dot) = PathBuilder::from_circle(cx, cy, 4.0 * k) {
                pm.fill_path(&dot, &paint(ERROR), FillRule::Winding, t, None);
            }
            x += icon_w + gap;
            let (bar_w, bar_gap) = (3.0 * k, 2.0 * k);
            for i in 0..6 {
                let v = levels.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                let bh = (3.0 + 11.0 * v) * k;
                let bx = x + i as f32 * (bar_w + bar_gap);
                if let Some(bar) = rounded_rect(bx, cy - bh / 2.0, bar_w, bh, bar_w / 2.0) {
                    pm.fill_path(&bar, &paint(ORBITA), FillRule::Winding, t, None);
                }
            }
            x += bars_w + gap;
        }
        PillState::Transcribing => {
            let (cx, rad) = (x + 7.0 * k, 5.0 * k);
            if let Some(ring) = PathBuilder::from_circle(cx, cy, rad) {
                pm.stroke_path(&ring, &paint(LINE_STRONG), &stroke(2.0 * k), t, None);
            }
            // un cuarto de vuelta que gira (ángulos como Pillow: en sentido horario desde las 3)
            let a0 = (spin * 360.0).rem_euclid(360.0).to_radians();
            let mut pb = PathBuilder::new();
            for step in 0..=24 {
                let a = a0 + std::f32::consts::FRAC_PI_2 * step as f32 / 24.0;
                let (px_, py_) = (cx + rad * a.cos(), cy + rad * a.sin());
                if step == 0 {
                    pb.move_to(px_, py_);
                } else {
                    pb.line_to(px_, py_);
                }
            }
            if let Some(arc) = pb.finish() {
                pm.stroke_path(&arc, &paint(ORBITA), &stroke(2.0 * k), t, None);
            }
            x += icon_w + gap;
        }
        PillState::Copied | PillState::Saved | PillState::Marked => {
            let mut pb = PathBuilder::new();
            pb.move_to(x + k, cy);
            pb.line_to(x + 5.5 * k, cy + 4.5 * k);
            pb.line_to(x + 13.0 * k, cy - 5.0 * k);
            if let Some(check) = pb.finish() {
                pm.stroke_path(&check, &paint(EXITO), &stroke(2.0 * k), t, None);
            }
            x += icon_w + gap;
        }
    }
    draw_text(&mut pm, &text, x, cy, px, INK);
    pm
}

/// (opacidad 0–255, desplazamiento hacia abajo en px) a los `t` segundos de aparecer (ease-out).
pub fn enter_frame(t: f64) -> (u8, i32) {
    if t >= ENTER_S {
        return (255, 0);
    }
    let k = t.max(0.0) / ENTER_S;
    let e = 1.0 - (1.0 - k).powi(3);
    (
        (255.0 * e).round() as u8,
        (f64::from(RISE_PX) * (1.0 - e)).round() as i32,
    )
}

/// Opacidad a los `t` segundos de empezar a desaparecer (ease-in).
pub fn exit_alpha(t: f64) -> u8 {
    if t >= EXIT_S {
        return 0;
    }
    let k = t.max(0.0) / EXIT_S;
    (255.0 * (1.0 - k * k)).round() as u8
}

/// "MM:SS", o "H:MM:SS" desde la hora.
pub fn fmt_clock(elapsed_s: f64) -> String {
    let s = elapsed_s.max(0.0) as u64;
    if s < 3600 {
        format!("{:02}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(p: &Pixmap, x: u32, y: u32) -> u8 {
        p.pixel(x, y).unwrap().alpha()
    }

    #[test]
    fn test_render_sizes() {
        let img = render_pill(PillState::Recording, &[0.2; 6], 42.0, 1.0, 0.0);
        assert!(img.width() < 160, "{}", img.width());
        assert!((30..=36).contains(&img.height()), "{}", img.height());
        let big = render_pill(PillState::Recording, &[0.2; 6], 42.0, 1.5, 0.0);
        assert!((45..=54).contains(&big.height()), "{}", big.height());
    }

    #[test]
    fn test_render_has_transparent_corners() {
        let img = render_pill(PillState::Recording, &[0.5; 6], 3.0, 1.0, 0.0);
        assert_eq!(alpha(&img, 0, 0), 0);
        assert!(alpha(&img, img.width() / 2, img.height() / 2) > 200);
    }

    #[test]
    fn test_render_copied_wider_than_recording() {
        assert!(
            render_pill(PillState::Copied, &[], 0.0, 1.0, 0.0).width()
                > render_pill(PillState::Recording, &[0.1; 6], 0.0, 1.0, 0.0).width()
        );
    }

    #[test]
    fn test_levels_change_image() {
        let a = render_pill(PillState::Recording, &[0.0; 6], 10.0, 1.0, 0.0);
        let b = render_pill(PillState::Recording, &[0.9; 6], 10.0, 1.0, 0.0);
        assert_ne!(a.data(), b.data());
    }

    #[test]
    fn test_transcribing_and_saved_render() {
        for state in [PillState::Transcribing, PillState::Saved] {
            assert!(render_pill(state, &[], 0.0, 1.0, 0.0).height() < 40);
        }
    }

    #[test]
    fn test_enter_frame_eases_in_and_settles() {
        assert_eq!(enter_frame(0.0), (0, RISE_PX));
        let (am, ym) = enter_frame(ENTER_S / 2.0);
        assert!(am > 127 && ym < RISE_PX / 2, "ease-out: {am} {ym}");
        assert_eq!(enter_frame(ENTER_S), (255, 0));
        assert_eq!(enter_frame(5.0), (255, 0));
    }

    #[test]
    fn test_exit_alpha() {
        assert_eq!(exit_alpha(0.0), 255);
        assert_eq!(exit_alpha(EXIT_S), 0);
        let mid = exit_alpha(EXIT_S / 2.0);
        assert!(mid > 0 && mid < 255);
    }

    #[test]
    fn test_marked_renders_its_text() {
        let marked = render_pill(PillState::Marked, &[], 0.0, 1.0, 0.0);
        let saved = render_pill(PillState::Saved, &[], 0.0, 1.0, 0.0);
        // "Marcado" es más corto que "Transcripción guardada"
        assert!(marked.width() < saved.width());
        assert!(marked.width() > 60);
    }

    /// Para mirarlas: `cargo test -p anotalo --bin anotalo dump_pills -- --ignored`
    #[test]
    #[ignore]
    fn dump_pills() {
        let dir = std::env::temp_dir().join("anotalo-pills");
        std::fs::create_dir_all(&dir).unwrap();
        let states = [
            ("recording", PillState::Recording),
            ("transcribing", PillState::Transcribing),
            ("copied", PillState::Copied),
            ("saved", PillState::Saved),
            ("marked", PillState::Marked),
        ];
        for (name, st) in states {
            let p = render_pill(st, &[0.1, 0.5, 0.9, 0.6, 0.3, 0.0], 83.0, 1.5, 0.1);
            p.save_png(dir.join(format!("{name}.png"))).unwrap();
        }
        println!("{}", dir.display());
    }

    #[test]
    fn test_fmt_clock() {
        assert_eq!(fmt_clock(0.0), "00:00");
        assert_eq!(fmt_clock(75.9), "01:15");
        assert_eq!(fmt_clock(3725.0), "1:02:05");
        assert_eq!(fmt_clock(-3.0), "00:00");
    }
}
