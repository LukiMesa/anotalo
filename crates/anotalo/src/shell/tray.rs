//! Icono en la bandeja del sistema (port de calltx/ui/tray.py): el icono de la app con un punto del
//! color del estado, su menú y el texto al pasar el ratón.

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
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    HICON, ICONINFO, MF_CHECKED, MF_SEPARATOR, MF_STRING, SetForegroundWindow, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
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
}

impl TrayCommand {
    pub fn from_id(id: i32) -> Option<TrayCommand> {
        [
            TrayCommand::ToggleNote,
            TrayCommand::RecordCall,
            TrayCommand::ToggleAuto,
            TrayCommand::OpenFolder,
            TrayCommand::Quit,
        ]
        .into_iter()
        .find(|c| *c as i32 == id)
    }
}

/// Una entrada del menú; `cmd: None` es un separador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub cmd: Option<TrayCommand>,
    pub text: &'static str,
    pub visible: bool,
    pub checked: bool,
}

fn item(cmd: TrayCommand, text: &'static str) -> MenuItem {
    MenuItem {
        cmd: Some(cmd),
        text,
        visible: true,
        checked: false,
    }
}

const SEPARATOR: MenuItem = MenuItem {
    cmd: None,
    text: "",
    visible: true,
    checked: false,
};

pub fn menu_items(state: State, auto_mode: bool) -> Vec<MenuItem> {
    let recording = state == State::Recording;
    vec![
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
        SEPARATOR,
        item(TrayCommand::Quit, "Salir"),
    ]
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

/// Icono de 64×64 (RGBA sin premultiplicar): el de la app con el punto del estado.
pub fn tray_icon_rgba(state: State) -> (u32, u32, Vec<u8>) {
    let mut pm = Pixmap::new(64, 64).expect("64×64");
    match Pixmap::decode_png(include_bytes!("../../../../calltx/ui/assets/icon.png")) {
        Ok(base) => {
            let (sx, sy) = (64.0 / base.width() as f32, 64.0 / base.height() as f32);
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
    // punto de estado abajo a la derecha: anillo azul marino (40..63) y el color (44..59)
    fill_circle(&mut pm, 52.0, 52.0, 12.0, NAVY);
    fill_circle(&mut pm, 52.0, 52.0, 8.0, state_color(state));
    let rgba = pm
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    (64, 64, rgba)
}

pub fn tray_tip(state: State, preparing_pct: Option<u8>) -> String {
    match (state, preparing_pct) {
        (State::Preparing, Some(p)) => format!("Anotalo — Preparando… {p} %"),
        _ => format!("Anotalo — {}", state_text(state)),
    }
}

/// HICON a partir del RGBA (color de 32 bits con alfa + máscara vacía).
fn make_hicon(state: State) -> Result<HICON, String> {
    let (w, h, rgba) = tray_icon_rgba(state);
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
                        let _ = AppendMenuW(menu, flags, cmd as usize, &HSTRING::from(it.text));
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
        let items = menu_items(State::Armed, true);
        let item = find(&items, TrayCommand::RecordCall);
        assert_eq!(item.text, "Grabar llamada");
        assert!(item.visible);
    }

    #[test]
    fn test_record_call_hidden_while_recording() {
        let items = menu_items(State::Recording, true);
        assert!(!find(&items, TrayCommand::RecordCall).visible);
        assert_eq!(
            find(&items, TrayCommand::ToggleNote).text,
            "Detener grabación"
        );
        assert_eq!(
            find(&menu_items(State::Idle, false), TrayCommand::ToggleNote).text,
            "Nueva nota rápida"
        );
    }

    #[test]
    fn test_auto_item_checked() {
        assert!(find(&menu_items(State::Armed, true), TrayCommand::ToggleAuto).checked);
        assert!(!find(&menu_items(State::Idle, false), TrayCommand::ToggleAuto).checked);
    }

    #[test]
    fn test_menu_order_and_separators() {
        let items = menu_items(State::Idle, true);
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
    fn test_icon_dot_colour_matches_state() {
        for (state, rgb) in [
            (State::Idle, (107, 125, 146)),
            (State::Armed, (46, 197, 230)),
            (State::Recording, (255, 122, 122)),
            (State::Preparing, (90, 104, 131)),
        ] {
            let (w, h, px) = tray_icon_rgba(state);
            assert_eq!((w, h), (64, 64));
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
