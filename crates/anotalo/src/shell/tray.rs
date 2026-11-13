//! Icono en la bandeja del sistema (port de calltx/ui/tray.py): el icono de la app con un punto del
//! color del estado, su menú y el texto al pasar el ratón. El icono se dibuja al tamaño de la
//! bandeja para el DPI del sistema, desde la base de ese tamaño (más nítido que escalar uno grande).

use std::cell::Cell;

use anotalo_core::app::State;
use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Transform,
};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HGDIOBJ,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetSystemMetricsForDpi,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    HICON, ICONINFO, MF_CHECKED, MF_SEPARATOR, MF_STRING, SM_CXSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
};
use windows::core::HSTRING;

/// Mensaje con el que la bandeja avisa a la ventana de la app (clic, menú).
pub const WM_APP_TRAY: u32 = WM_APP + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ToggleNote = 1,
    RecordCall = 2,
    ToggleAuto = 3,
    OpenFolder = 4,
    Quit = 5,
    /// "Instalar la versión X", mientras hay una actualización disponible.
    InstallUpdate = 6,
}

impl TrayCommand {
    pub fn from_id(id: i32) -> Option<TrayCommand> {
        [
            TrayCommand::ToggleNote,
            TrayCommand::RecordCall,
            TrayCommand::ToggleAuto,
            TrayCommand::OpenFolder,
            TrayCommand::Quit,
            TrayCommand::InstallUpdate,
        ]
        .into_iter()
        .find(|c| *c as i32 == id)
    }
}

/// Una entrada del menú; `cmd: None` es un separador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub cmd: Option<TrayCommand>,
    pub text: String,
    pub visible: bool,
    pub checked: bool,
}

fn item(cmd: TrayCommand, text: impl Into<String>) -> MenuItem {
    MenuItem {
        cmd: Some(cmd),
        text: text.into(),
        visible: true,
        checked: false,
    }
}

const SEPARATOR: MenuItem = MenuItem {
    cmd: None,
    text: String::new(),
    visible: true,
    checked: false,
};

/// El menú. `update`: la versión que se puede instalar (su entrada va encima del separador que
/// precede a "Salir"; también mientras se graba: instalarla espera a que termine).
pub fn menu_items(state: State, auto_mode: bool, update: Option<&str>) -> Vec<MenuItem> {
    let recording = state == State::Recording;
    let mut items = vec![
        item(
            TrayCommand::ToggleNote,
            if recording {
                "Detener grabación"
            } else {
                "Nueva nota rápida"
            },
        ),
        MenuItem {
            visible: !recording, // mientras graba, "Detener grabación" ya lo para
            ..item(TrayCommand::RecordCall, "Grabar llamada")
        },
        MenuItem {
            checked: auto_mode,
            ..item(TrayCommand::ToggleAuto, "Grabar llamadas automáticamente")
        },
        SEPARATOR,
        item(TrayCommand::OpenFolder, "Abrir carpeta de grabaciones"),
    ];
    if let Some(v) = update {
        items.push(item(
            TrayCommand::InstallUpdate,
            format!("Instalar la versión {v}"),
        ));
    }
    items.extend([SEPARATOR, item(TrayCommand::Quit, "Salir")]);
    items
}

/// Colores del tema: acero oscuro (inactivo), órbita (esperando), error (grabando), como Python.
fn state_color(state: State) -> (u8, u8, u8) {
    match state {
        State::Idle => (107, 125, 146),
        State::Armed => (46, 197, 230),
        State::Recording => (255, 122, 122),
        State::Preparing => (90, 104, 131),
    }
}

fn state_text(state: State) -> &'static str {
    match state {
        State::Idle => "Inactivo",
        State::Armed => "Esperando llamada",
        State::Recording => "Grabando",
        State::Preparing => "Preparando…",
    }
}

const NAVY: (u8, u8, u8) = (22, 28, 43);

fn fill_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, c: (u8, u8, u8)) {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(c.0, c.1, c.2, 255));
    paint.anti_alias = true;
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// El icono de Anotalo dibujado para la bandeja a cada tamaño (16, 24 y 32 con la geometría
/// pequeña, más gruesa).
const BASES: [(u32, &[u8]); 5] = [
    (16, include_bytes!("../../assets/tray-base-16.png")),
    (20, include_bytes!("../../assets/tray-base-20.png")),
    (24, include_bytes!("../../assets/tray-base-24.png")),
    (32, include_bytes!("../../assets/tray-base-32.png")),
    (64, include_bytes!("../../assets/tray-base-64.png")),
];

/// El tamaño de los iconos de la bandeja para el DPI del sistema (16 px al 100 %).
fn tray_size() -> u32 {
    // SAFETY: cambia el modo DPI del hilo un momento (para leer el DPI real) y lo deja como estaba.
    let n = unsafe {
        let before = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let n = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem());
        if !before.0.is_null() {
            SetThreadDpiAwarenessContext(before);
        }
        n
    };
    u32::try_from(n).ok().filter(|n| *n > 0).unwrap_or(16)
}

/// El icono de `size`×`size` (RGBA sin premultiplicar): el de la app con el punto del estado abajo
/// a la derecha (centro a 13/16, anillo azul marino de 3/16 y el color en 2/16).
pub fn tray_icon_rgba(state: State, size: u32) -> (u32, u32, Vec<u8>) {
    let size = size.clamp(16, 256);
    let mut pm = Pixmap::new(size, size).expect("tamaño del icono");
    let (n, png) = BASES
        .iter()
        .find(|(n, _)| *n >= size)
        .unwrap_or(&BASES[BASES.len() - 1]);
    match Pixmap::decode_png(png) {
        Ok(base) => {
            let s = size as f32 / *n as f32;
            let (sx, sy) = (s, s);
            pm.draw_pixmap(
                0,
                0,
                base.as_ref(),
                &PixmapPaint {
                    quality: FilterQuality::Bicubic,
                    ..PixmapPaint::default()
                },
                Transform::from_scale(sx, sy),
                None,
            );
        }
        Err(_) => pm.fill(Color::from_rgba8(NAVY.0, NAVY.1, NAVY.2, 255)),
    }
    let u = size as f32 / 16.0;
    fill_circle(&mut pm, 13.0 * u, 13.0 * u, 3.0 * u, NAVY);
    fill_circle(&mut pm, 13.0 * u, 13.0 * u, 2.0 * u, state_color(state));
    let rgba = pm
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    (size, size, rgba)
}

pub fn tray_tip(state: State, preparing_pct: Option<u8>) -> String {
    match (state, preparing_pct) {
        (State::Preparing, Some(p)) => format!("Anotalo — Preparando… {p} %"),
        _ => format!("Anotalo — {}", state_text(state)),
    }
}

/// HICON a partir del RGBA (color de 32 bits con alfa + máscara vacía).
fn make_hicon(state: State) -> Result<HICON, String> {
    let (w, h, rgba) = tray_icon_rgba(state, tray_size());
    // SAFETY: mapas de bits creados aquí y borrados tras crear el icono (que hace su copia).
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| e.to_string())?;
        let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (w * h * 4) as usize);
        let (dst4, _) = dst.as_chunks_mut::<4>();
        let (src4, _) = rgba.as_chunks::<4>();
        for (d, s) in dst4.iter_mut().zip(src4) {
            *d = [s[2], s[1], s[0], s[3]];
        }
        let mask = CreateBitmap(w as i32, h as i32, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).map_err(|e| e.to_string());
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}

fn tip_buf(text: &str) -> [u16; 128] {
    let mut buf = [0u16; 128];
    for (d, c) in buf.iter_mut().zip(text.encode_utf16().take(127)) {
        *d = c;
    }
    buf
}

/// El icono en la bandeja. Vive en el hilo de la ventana de mensajes. Sus métodos toman `&self`:
/// el menú abre un bucle de mensajes propio, y los avisos que llegan mientras pueden actualizarlo.
pub struct Tray {
    hwnd: HWND,
    icon: Cell<HICON>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Result<Tray, String> {
        let icon = make_hicon(State::Idle)?;
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: icon,
            szTip: tip_buf("Anotalo"),
            ..Default::default()
        };
        // SAFETY: estructura completa con su tamaño; la ventana es de este hilo.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            // SAFETY: icono propio.
            let _ = unsafe { DestroyIcon(icon) };
            return Err("no se pudo añadir el icono a la bandeja".into());
        }
        Ok(Tray {
            hwnd,
            icon: Cell::new(icon),
        })
    }

    pub fn update(&self, state: State, preparing_pct: Option<u8>) {
        let Ok(icon) = make_hicon(state) else { return };
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_TIP,
            hIcon: icon,
            szTip: tip_buf(&tray_tip(state, preparing_pct)),
            ..Default::default()
        };
        // SAFETY: ver new; el icono anterior se destruye cuando ya no se usa.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
            let _ = DestroyIcon(self.icon.replace(icon));
        }
    }

    /// Muestra el menú donde está el ratón y devuelve lo elegido.
    pub fn show_menu(&self, items: &[MenuItem]) -> Option<TrayCommand> {
        // SAFETY: menú creado y destruido aquí, en el hilo de la ventana.
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            for it in items.iter().filter(|i| i.visible) {
                match it.cmd {
                    None => {
                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                    }
                    Some(cmd) => {
                        let flags = if it.checked {
                            MF_STRING | MF_CHECKED
                        } else {
                            MF_STRING
                        };
                        let _ = AppendMenuW(
                            menu,
                            flags,
                            cmd as usize,
                            &HSTRING::from(it.text.as_str()),
                        );
                    }
                }
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            let id = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            TrayCommand::from_id(id.0)
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        };
        // SAFETY: quita el icono propio y lo destruye.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            let _ = DestroyIcon(self.icon.get());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(items: &[MenuItem], cmd: TrayCommand) -> MenuItem {
        items.iter().find(|i| i.cmd == Some(cmd)).unwrap().clone()
    }

    #[test]
    fn test_tray_has_record_call_item() {
        let items = menu_items(State::Armed, true, None);
        let item = find(&items, TrayCommand::RecordCall);
        assert_eq!(item.text, "Grabar llamada");
        assert!(item.visible);
    }

    #[test]
    fn test_record_call_hidden_while_recording() {
        let items = menu_items(State::Recording, true, None);
        assert!(!find(&items, TrayCommand::RecordCall).visible);
        assert_eq!(
            find(&items, TrayCommand::ToggleNote).text,
            "Detener grabación"
        );
        assert_eq!(
            find(
                &menu_items(State::Idle, false, None),
                TrayCommand::ToggleNote
            )
            .text,
            "Nueva nota rápida"
        );
    }

    #[test]
    fn test_auto_item_checked() {
        assert!(
            find(
                &menu_items(State::Armed, true, None),
                TrayCommand::ToggleAuto
            )
            .checked
        );
        assert!(
            !find(
                &menu_items(State::Idle, false, None),
                TrayCommand::ToggleAuto
            )
            .checked
        );
    }

    #[test]
    fn test_menu_order_and_separators() {
        let items = menu_items(State::Idle, true, None);
        let order: Vec<Option<TrayCommand>> = items.iter().map(|i| i.cmd).collect();
        assert_eq!(
            order,
            vec![
                Some(TrayCommand::ToggleNote),
                Some(TrayCommand::RecordCall),
                Some(TrayCommand::ToggleAuto),
                None,
                Some(TrayCommand::OpenFolder),
                None,
                Some(TrayCommand::Quit),
            ]
        );
        assert_eq!(find(&items, TrayCommand::Quit).text, "Salir");
        assert_eq!(
            find(&items, TrayCommand::OpenFolder).text,
            "Abrir carpeta de grabaciones"
        );
    }

    #[test]
    fn test_update_item_only_when_available() {
        let has = |items: &[MenuItem]| items.iter().any(|i| i.text == "Instalar la versión 1.0.1");
        assert!(has(&menu_items(State::Idle, true, Some("1.0.1"))));
        assert!(has(&menu_items(State::Recording, true, Some("1.0.1"))));
        assert!(
            !menu_items(State::Idle, true, None)
                .iter()
                .any(|i| i.text.starts_with("Instalar"))
        );
        let items = menu_items(State::Idle, true, Some("1.0.1"));
        let pos = items
            .iter()
            .position(|i| i.text == "Instalar la versión 1.0.1")
            .unwrap();
        assert_eq!(items.last().unwrap().text, "Salir");
        assert!(pos < items.len() - 2);
        // justo encima del separador que va antes de "Salir"
        assert_eq!(items[pos + 1].cmd, None);
        assert_eq!(items[pos].cmd, Some(TrayCommand::InstallUpdate));
        assert_eq!(
            TrayCommand::from_id(TrayCommand::InstallUpdate as i32),
            Some(TrayCommand::InstallUpdate)
        );
    }

    #[test]
    fn test_icon_dot_colour_matches_state() {
        for (state, rgb) in [
            (State::Idle, (107, 125, 146)),
            (State::Armed, (46, 197, 230)),
            (State::Recording, (255, 122, 122)),
            (State::Preparing, (90, 104, 131)),
        ] {
            let (w, h, px) = tray_icon_rgba(state, 64);
            assert_eq!((w, h), (64, 64));
            // la base es el icono de Anotalo: una de sus barras claras
            let k = ((20 * w + 28) * 4) as usize;
            assert_eq!(
                (px[k], px[k + 1], px[k + 2], px[k + 3]),
                (238, 243, 247, 255),
                "{state:?}"
            );
            let i = ((51 * w + 51) * 4) as usize;
            assert_eq!(
                (px[i], px[i + 1], px[i + 2], px[i + 3]),
                (rgb.0, rgb.1, rgb.2, 255),
                "{state:?}"
            );
            // el anillo azul marino alrededor del punto
            let j = ((51 * w + 42) * 4) as usize;
            assert_eq!((px[j], px[j + 1], px[j + 2]), (22, 28, 43), "{state:?}");
        }
    }

    /// Se dibuja al tamaño de la bandeja (16 px a 100 %, 20 a 125 %, 24 a 150 %, 32 a 200 %), con el
    /// punto del estado en el mismo sitio (centro a 13/16, anillo de 3/16 y punto de 2/16).
    #[test]
    fn test_icon_drawn_at_tray_size() {
        for n in [16u32, 20, 24, 32, 40] {
            let (w, h, px) = tray_icon_rgba(State::Recording, n);
            assert_eq!((w, h), (n, n));
            let c = (n * 13 / 16) as usize;
            let i = (c * n as usize + c) * 4;
            assert_eq!((px[i], px[i + 1], px[i + 2]), (255, 122, 122), "{n} px");
        }
    }

    /// Para mirarlos: `cargo test -p anotalo --bin anotalo dump_tray_icons -- --ignored`
    #[test]
    #[ignore]
    fn dump_tray_icons() {
        let dir = std::env::temp_dir().join("anotalo-tray");
        std::fs::create_dir_all(&dir).unwrap();
        for n in [16u32, 20, 24, 32] {
            for (name, st) in [
                ("idle", State::Idle),
                ("armed", State::Armed),
                ("recording", State::Recording),
                ("preparing", State::Preparing),
            ] {
                let (w, h, rgba) = tray_icon_rgba(st, n);
                let mut pm = Pixmap::new(w, h).unwrap();
                for (d, s) in pm.pixels_mut().iter_mut().zip(rgba.as_chunks::<4>().0) {
                    *d = tiny_skia::ColorU8::from_rgba(s[0], s[1], s[2], s[3]).premultiply();
                }
                pm.save_png(dir.join(format!("{n}-{name}.png"))).unwrap();
            }
        }
        println!("{}", dir.display());
    }

    #[test]
    fn test_tip_texts() {
        assert_eq!(tray_tip(State::Idle, None), "Anotalo — Inactivo");
        assert_eq!(tray_tip(State::Armed, None), "Anotalo — Esperando llamada");
        assert_eq!(tray_tip(State::Recording, None), "Anotalo — Grabando");
        assert_eq!(tray_tip(State::Preparing, None), "Anotalo — Preparando…");
        assert_eq!(
            tray_tip(State::Preparing, Some(42)),
            "Anotalo — Preparando… 42 %"
        );
    }

    #[test]
    #[ignore]
    fn tray_icon_appears() {
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
        };
        use windows::core::w;
        // SAFETY: ventana de mensajes de este hilo, destruida al acabar.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap()
        };
        let tray = Tray::new(hwnd).expect("icono en la bandeja");
        for state in [State::Armed, State::Recording, State::Preparing] {
            tray.update(state, Some(42));
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        drop(tray);
        // SAFETY: ver arriba.
        unsafe { DestroyWindow(hwnd).unwrap() };
    }
}
