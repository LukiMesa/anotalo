//! Las cuentas del marco de la ventana principal (port de calltx/ui/nativeframe.py), sin Windows.
//!
//! La ventana conserva los estilos del marco de Windows (título, borde redimensionable, minimizar,
//! maximizar), así el propio Windows anima minimizar y maximizar y funciona Snap. `WM_NCCALCSIZE`
//! quita el marco (todo es contenido) y `WM_NCHITTEST` convierte el borde fino que deja la página
//! en bordes de redimensionado nativos.

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
};

/// Borde para redimensionar alrededor de la página, en píxeles a 96 ppp (se escala con el DPI).
pub const RESIZE_BORDER: i32 = 5;

/// Los bits de estilo que Windows necesita para animar la ventana y encajarla.
pub const FRAME_BITS: u32 =
    WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;

pub const SM_CXSIZEFRAME: i32 = 32;
pub const SM_CYSIZEFRAME: i32 = 33;
pub const SM_CXPADDEDBORDER: i32 = 92;

/// El estilo con los bits de marco que Windows necesita para animar la ventana.
pub fn keep_frame_style(style: u32) -> u32 {
    style | FRAME_BITS
}

/// Grosor del marco que Windows deja fuera de la pantalla al maximizar, para este DPI.
/// `metric` es `GetSystemMetricsForDpi`.
pub fn frame_inset(dpi: u32, metric: impl Fn(i32, u32) -> i32) -> (i32, i32) {
    let pad = metric(SM_CXPADDEDBORDER, dpi);
    (
        metric(SM_CXSIZEFRAME, dpi) + pad,
        metric(SM_CYSIZEFRAME, dpi) + pad,
    )
}

/// Zona de contenido: toda la ventana (sin marco); maximizada, sin lo que sobresale.
pub fn client_rect(proposed: RECT, maximized: bool, inset: (i32, i32)) -> RECT {
    if !maximized {
        return proposed;
    }
    let (ix, iy) = inset;
    RECT {
        left: proposed.left + ix,
        top: proposed.top + iy,
        right: proposed.right - ix,
        bottom: proposed.bottom - iy,
    }
}

/// Qué parte de la ventana hay en (x, y), relativo a su esquina: borde o contenido. Las esquinas
/// ocupan 3 veces el borde a lo largo de cada lado. Maximizada no hay bordes.
pub fn hit_test(x: i32, y: i32, w: i32, h: i32, border: i32, maximized: bool) -> u32 {
    if maximized {
        return HTCLIENT;
    }
    let c = border * 3;
    let (top, bottom) = (y < c, y >= h - c);
    let (left, right) = (x < c, x >= w - c);
    if top && left {
        HTTOPLEFT
    } else if top && right {
        HTTOPRIGHT
    } else if bottom && left {
        HTBOTTOMLEFT
    } else if bottom && right {
        HTBOTTOMRIGHT
    } else if x < border {
        HTLEFT
    } else if x >= w - border {
        HTRIGHT
    } else if y < border {
        HTTOP
    } else if y >= h - border {
        HTBOTTOM
    } else {
        HTCLIENT
    }
}

/// Dónde abrir la ventana. `saved` es donde estaba; `work_areas`, las áreas de trabajo de los
/// monitores, la del principal primero. Si no hay nada guardado o ya no se ve en ningún monitor,
/// `default` centrado en el principal. Si se ve, en el área donde más se ve: al menos `min` y
/// dentro de esa área. Nunca más grande que el área de trabajo.
pub fn restore_rect(
    saved: Option<RECT>,
    work_areas: &[RECT],
    default: (i32, i32),
    min: (i32, i32),
) -> RECT {
    let Some(&primary) = work_areas.first() else {
        return saved.unwrap_or(RECT {
            left: 0,
            top: 0,
            right: default.0,
            bottom: default.1,
        });
    };
    let visible = saved.and_then(|s| {
        work_areas
            .iter()
            .map(|a| (overlap(&s, a), *a))
            .filter(|&(o, _)| o > 0)
            .max_by_key(|&(o, _)| o)
            .map(|(_, a)| (s, a))
    });
    match visible {
        Some((s, area)) => fit(
            s.left,
            s.top,
            (s.right - s.left).max(min.0),
            (s.bottom - s.top).max(min.1),
            area,
        ),
        None => {
            let (w, h) = (
                default.0.min(primary.right - primary.left),
                default.1.min(primary.bottom - primary.top),
            );
            fit(
                primary.left + (primary.right - primary.left - w) / 2,
                primary.top + (primary.bottom - primary.top - h) / 2,
                w,
                h,
                primary,
            )
        }
    }
}

/// Píxeles que comparten dos rectángulos.
fn overlap(a: &RECT, b: &RECT) -> i64 {
    let w = i64::from(a.right.min(b.right) - a.left.max(b.left));
    let h = i64::from(a.bottom.min(b.bottom) - a.top.max(b.top));
    if w > 0 && h > 0 { w * h } else { 0 }
}

/// Un rectángulo de w×h en (left, top), movido y recortado hasta caber en `area`.
fn fit(left: i32, top: i32, w: i32, h: i32, area: RECT) -> RECT {
    let w = w.min(area.right - area.left);
    let h = h.min(area.bottom - area.top);
    let left = left.clamp(area.left, area.right - w);
    let top = top.clamp(area.top, area.bottom - h);
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn test_keep_frame_style_restores_bits() {
        assert_eq!(keep_frame_style(0x1600_0000) & FRAME_BITS, FRAME_BITS);
        assert_eq!(keep_frame_style(0x1600_0000) & 0x1600_0000, 0x1600_0000);
    }

    #[test]
    fn test_frame_inset_uses_dpi() {
        let seen = RefCell::new(Vec::new());
        let metric = |i: i32, dpi: u32| {
            seen.borrow_mut().push((i, dpi));
            let base = match i {
                SM_CXSIZEFRAME | SM_CYSIZEFRAME => 4,
                SM_CXPADDEDBORDER => 8,
                other => panic!("métrica inesperada {other}"),
            };
            base * dpi as i32 / 96
        };
        assert_eq!(frame_inset(96, metric), (12, 12));
        assert!(seen.borrow().iter().all(|&(_, d)| d == 96));
        seen.borrow_mut().clear();
        assert_eq!(frame_inset(192, metric), (24, 24));
        assert!(seen.borrow().iter().all(|&(_, d)| d == 192));
    }

    #[test]
    fn test_client_rect_normal_and_maximized() {
        assert_eq!(
            client_rect(rect(0, 0, 1000, 700), false, (8, 8)),
            rect(0, 0, 1000, 700)
        );
        assert_eq!(
            client_rect(rect(-8, -8, 2568, 1400), true, (8, 8)),
            rect(0, 0, 2560, 1392)
        );
    }

    #[test]
    fn test_hit_test_zones() {
        let (w, h, b) = (1000, 700, 5);
        assert_eq!(hit_test(500, 300, w, h, b, false), HTCLIENT);
        assert_eq!(hit_test(2, 300, w, h, b, false), HTLEFT);
        assert_eq!(hit_test(998, 300, w, h, b, false), HTRIGHT);
        assert_eq!(hit_test(500, 1, w, h, b, false), HTTOP);
        assert_eq!(hit_test(500, 698, w, h, b, false), HTBOTTOM);
        assert_eq!(hit_test(3, 3, w, h, b, false), HTTOPLEFT);
        assert_eq!(hit_test(997, 3, w, h, b, false), HTTOPRIGHT);
        assert_eq!(hit_test(3, 697, w, h, b, false), HTBOTTOMLEFT);
        assert_eq!(hit_test(997, 697, w, h, b, false), HTBOTTOMRIGHT);
        // las esquinas ocupan 3 veces el borde a lo largo de cada lado
        assert_eq!(hit_test(2, 14, w, h, b, false), HTTOPLEFT);
        assert_eq!(hit_test(2, 15, w, h, b, false), HTLEFT);
        // los valores de Windows, como en Python
        assert_eq!(
            (HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTBOTTOM),
            (1, 10, 11, 12, 15)
        );
        assert_eq!((HTTOPLEFT, HTBOTTOMRIGHT), (13, 17));
        // maximizada no hay bordes
        for (x, y) in [(2, 300), (3, 3), (997, 697), (500, 1), (500, 300)] {
            assert_eq!(hit_test(x, y, w, h, b, true), HTCLIENT);
        }
    }

    #[test]
    fn test_restore_rect_clamped_to_work_area() {
        // la principal primero; la segunda, a la derecha
        let areas = [rect(0, 0, 1920, 1040), rect(1920, 0, 3840, 1040)];
        let (default, min) = ((1120, 720), (940, 600));
        let centred = rect(400, 160, 1520, 880);
        // sin nada guardado, o guardada en un monitor que ya no está: centrada en la principal
        assert_eq!(restore_rect(None, &areas, default, min), centred);
        assert_eq!(
            restore_rect(Some(rect(-3000, 100, -1880, 820)), &areas, default, min),
            centred
        );
        // donde estaba, si se ve
        let there = rect(2000, 50, 3200, 850);
        assert_eq!(restore_rect(Some(there), &areas, default, min), there);
        // más pequeña que el mínimo: crece hasta 940×600
        assert_eq!(
            restore_rect(Some(rect(100, 100, 600, 400)), &areas, default, min),
            rect(100, 100, 1040, 700)
        );
        // asomando por fuera: vuelve dentro del área de trabajo donde más se ve
        assert_eq!(
            restore_rect(Some(rect(3500, 100, 4620, 820)), &areas, default, min),
            rect(2720, 100, 3840, 820)
        );
        // la barra de tareas arriba: el área empieza más abajo
        let top_bar = [rect(0, 40, 1920, 1080)];
        assert_eq!(
            restore_rect(None, &top_bar, default, min),
            rect(400, 200, 1520, 920)
        );
        // pantalla pequeña: nunca más grande que el área de trabajo
        let small = [rect(0, 0, 1024, 728)];
        assert_eq!(
            restore_rect(None, &small, default, min),
            rect(0, 4, 1024, 724)
        );
    }
}
