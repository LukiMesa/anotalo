//! La ventana principal (port de calltx/ui/window.py, nativeframe.py y winchrome.py).
//!
//! Es una ventana Win32 propia del hilo principal, sin marco a la vista pero con el comportamiento
//! del de Windows (animaciones de minimizar y maximizar, Snap, bordes para redimensionar: ver
//! `frame.rs`). Dentro va la página en WebView2 a través de `wry`, como ventana hija que deja libre
//! el borde fino para redimensionar. La página se sirve desde el exe (`anotalo://localhost/…`) y habla
//! con Rust por `window.ipc` (ver `ui_bridge.rs` y `web/bridge.js`). Cerrarla solo la oculta.
//! WebView2 se carga con nuestro cargador (`anotalo_winsys::webview2_loader`), no con el de
//! Microsoft.
//!
//! Todo lo de la ventana es "consciente del DPI por monitor" (v2), aunque el resto del hilo no lo
//! sea: las coordenadas que entran y salen (`create`, `rect`, `work_areas`) son píxeles físicos.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::num::NonZeroIsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use anotalo_core::paths;
use anotalo_winsys::webview2_loader;
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings9;
use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, RPC_E_CHANGED_MODE,
    WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
    GetDpiForWindow, GetSystemMetricsForDpi, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_STYLE,
    GWLP_USERDATA, GetClientRect, GetSystemMetrics, GetWindowLongPtrW, GetWindowPlacement,
    GetWindowRect, HICON, HTCLIENT, IDC_ARROW, IMAGE_ICON, IsIconic, IsWindowVisible, IsZoomed,
    LR_DEFAULTSIZE, LoadCursorW, LoadImageW, MINMAXINFO, MONITORINFOF_PRIMARY, NCCALCSIZE_PARAMS,
    RegisterClassExW, SIZE_MAXIMIZED, SIZE_MINIMIZED, SIZE_RESTORED, SM_CXSMICON, SM_CYSMICON,
    STYLESTRUCT, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SYSTEM_METRICS_INDEX,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WINDOWPLACEMENT, WM_CLOSE, WM_DPICHANGED, WM_GETMINMAXINFO, WM_MOVE, WM_NCCALCSIZE,
    WM_NCDESTROY, WM_NCHITTEST, WM_SETFOCUS, WM_SHOWWINDOW, WM_SIZE, WM_STYLECHANGING, WNDCLASSEXW,
    WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW,
};
use windows::core::{BOOL, Interface, PCWSTR, w};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::http::{Response, StatusCode, Uri, header};
use wry::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use wry::{
    NewWindowResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows,
    WebViewExtWindows,
};

use super::frame::{self, RESIZE_BORDER};

/// Clase de la ventana (la buscan los tests de punta a punta).
pub const CLASS: PCWSTR = w!("AnotaloMainWindow");
/// Tamaño la primera vez y mínimo, en píxeles a 96 ppp (los de la app de Python).
pub const DEFAULT_SIZE: (i32, i32) = (1120, 720);
pub const MIN_SIZE: (i32, i32) = (940, 600);
/// Fondo de la ventana y de la página mientras carga: #10141F.
const BACKGROUND: (u8, u8, u8) = (0x10, 0x14, 0x1F);
/// La página, servida desde el exe.
pub const START_URL: &str = "anotalo://localhost/index.html";
const PROTOCOL: &str = "anotalo";
/// Cómo la ve WebView2: wry convierte `anotalo://localhost/` en `https://anotalo.localhost/`, un
/// origen seguro (portapapeles) que nunca sale a internet (`.localhost` no se resuelve fuera del
/// equipo), a diferencia de un dominio de verdad.
const PAGE_ORIGIN: &str = "https://anotalo.localhost/";

mod assets {
    include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));
}

/// El tipo de contenido por la extensión del archivo.
fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// La ruta de la URL sin los `%XX` (una secuencia mal formada se deja tal cual).
fn decode_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// El archivo de la página para una ruta (`/` es `index.html`): su tipo y su contenido.
fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    let name = match path.trim_start_matches('/') {
        "" => "index.html",
        name => name,
    };
    assets::ASSETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(n, body)| (content_type(n), *body))
}

/// La respuesta a una petición `anotalo://localhost/<ruta>`.
fn serve(uri: &str) -> Response<Cow<'static, [u8]>> {
    let path = uri
        .parse::<Uri>()
        .map(|u| decode_path(u.path()))
        .unwrap_or_default();
    let response = match asset(&path) {
        Some((ty, body)) => Response::builder()
            .header(header::CONTENT_TYPE, ty)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Cow::Borrowed(body)),
        None => {
            log::warn!("La página pidió algo que no existe: {path}");
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Cow::Borrowed(&b"No encontrado"[..]))
        }
    };
    response.unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

/// El hilo, en modo "consciente del DPI por monitor" (v2) mientras viva.
pub(crate) struct PerMonitorDpi(DPI_AWARENESS_CONTEXT);

impl PerMonitorDpi {
    pub(crate) fn enter() -> PerMonitorDpi {
        // SAFETY: cambia el modo del hilo; Drop lo deja como estaba.
        PerMonitorDpi(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        })
    }
}

impl Drop for PerMonitorDpi {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: el modo que tenía el hilo.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

fn scaled(px: i32, dpi: u32) -> i32 {
    (px * dpi as i32 + 48) / 96
}

fn metric_for_dpi(index: i32, dpi: u32) -> i32 {
    // SAFETY: sin punteros.
    unsafe { GetSystemMetricsForDpi(SYSTEM_METRICS_INDEX(index), dpi) }
}

/// DPI de la ventana (96 si no se sabe).
fn dpi_of(hwnd: HWND) -> u32 {
    // SAFETY: sin punteros.
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    }
}

/// Las áreas de trabajo de los monitores, la del principal primero (para `frame::restore_rect`).
pub fn work_areas() -> Vec<RECT> {
    unsafe extern "system" fn add(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` es el Vec de `work_areas`, vivo durante la enumeración.
        let areas = unsafe { &mut *(data.0 as *mut Vec<(bool, RECT)>) };
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `mi` con su tamaño.
        if unsafe { GetMonitorInfoW(m, &mut mi) }.as_bool() {
            areas.push((mi.dwFlags & MONITORINFOF_PRIMARY != 0, mi.rcWork));
        }
        true.into()
    }
    let _dpi = PerMonitorDpi::enter();
    let mut areas: Vec<(bool, RECT)> = Vec::new();
    // SAFETY: el callback solo usa `areas`, que vive hasta el final.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut areas as isize));
    }
    areas.sort_by_key(|(primary, _)| !primary);
    areas.into_iter().map(|(_, r)| r).collect()
}

/// Dónde abrir la ventana: donde estaba (`saved`) si se ve, si no centrada en el monitor principal
/// con el tamaño por defecto (a la escala del sistema).
pub fn initial_rect(saved: Option<RECT>) -> RECT {
    let areas = work_areas();
    let dpi = {
        let _dpi = PerMonitorDpi::enter();
        // SAFETY: sin punteros.
        unsafe { GetDpiForSystem() }
    };
    let size = |(w, h): (i32, i32)| (scaled(w, dpi), scaled(h, dpi));
    frame::restore_rect(saved, &areas, size(DEFAULT_SIZE), size(MIN_SIZE))
}

/// Lo que cambia en la ventana y le interesa a la app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowChange {
    /// Se ocultó (su botón de cerrar, Alt+F4, `hide`).
    Hidden,
    /// Se maximizó (true) o volvió a su tamaño (false), por el botón, Snap o un doble clic.
    Maximized(bool),
}

/// Quien se entera de los cambios. Se llama dentro del procedimiento de la ventana: que solo
/// anote o publique un mensaje.
pub type Listener = Box<dyn Fn(WindowChange)>;

/// Lo que el procedimiento de la ventana necesita. Vive en el montón y su dirección va en
/// `GWLP_USERDATA` mientras exista la ventana.
struct Shared {
    webview: RefCell<Option<WebView>>,
    listener: RefCell<Option<Listener>>,
    maximized: Cell<bool>,
}

impl Shared {
    fn tell(&self, change: WindowChange) {
        if let Ok(l) = self.listener.try_borrow()
            && let Some(l) = l.as_ref()
        {
            l(change);
        }
    }
}

/// La ventana principal. Solo del hilo que la crea (el principal), que debe mover sus mensajes.
pub struct MainWindow {
    hwnd: HWND,
    shared: Box<Shared>,
}

impl MainWindow {
    /// Crea la ventana y la página. `on_ipc` recibe cada mensaje de la página (en este hilo).
    /// `rect`: dónde, en píxeles físicos (ver `initial_rect`). `start_hidden`: `--minimized`.
    /// Sin WebView2, el error dice de dónde descargarlo (`webview2_loader::NOT_FOUND`).
    pub fn create(
        on_ipc: Box<dyn Fn(String)>,
        start_hidden: bool,
        rect: RECT,
    ) -> Result<MainWindow, String> {
        let runtime = webview2_loader::find_runtime()?;
        log::info!(
            "WebView2 {} ({})",
            runtime.version().map_or("?".into(), |v| v.to_string()),
            runtime.dll.display()
        );
        // WebView2 necesita un apartamento COM de un solo hilo
        // SAFETY: se queda iniciado lo que viva el hilo (la ventana vive lo que la app).
        if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) } == RPC_E_CHANGED_MODE {
            return Err("La ventana necesita COM de un solo hilo (STA) en su hilo".into());
        }
        let _dpi = PerMonitorDpi::enter();
        let hwnd = create_hwnd(rect)?;
        let window = MainWindow {
            hwnd,
            shared: Box::new(Shared {
                webview: RefCell::new(None),
                listener: RefCell::new(None),
                maximized: Cell::new(false),
            }),
        };
        // SAFETY: `shared` vive en el montón hasta Drop, que antes la quita de la ventana.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &raw const *window.shared as isize);
            // que Windows vuelva a calcular el marco con nuestro WM_NCCALCSIZE
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        apply_chrome(hwnd);
        let webview = build_webview(hwnd, on_ipc)?; // si falla, Drop destruye la ventana
        enable_drag_regions(&webview);
        webview
            .load_url(&START_URL.replacen("anotalo://", "https://anotalo.", 1))
            .map_err(|e| format!("No se pudo abrir la página: {e}"))?;
        *window.shared.webview.borrow_mut() = Some(webview);
        fit_webview(hwnd);
        if !start_hidden {
            window.show();
        }
        Ok(window)
    }

    /// Quién se entera de que se oculta o se maximiza.
    pub fn set_listener(&self, listener: Listener) {
        *self.shared.listener.borrow_mut() = Some(listener);
    }

    /// La muestra (restaurada si estaba minimizada) y la trae al frente.
    pub fn show(&self) {
        // SAFETY: ventana de este hilo.
        unsafe {
            let cmd = if IsIconic(self.hwnd).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            let _ = ShowWindow(self.hwnd, cmd);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hide(&self) {
        // SAFETY: ventana de este hilo.
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub fn minimize(&self) {
        // SAFETY: ventana de este hilo.
        let _ = unsafe { ShowWindow(self.hwnd, SW_MINIMIZE) };
    }

    /// Maximizar es el de Windows (así encajar, doble clic y Win+↓ son nativos). Devuelve si
    /// queda maximizada.
    pub fn toggle_maximize(&self) -> bool {
        let maximized = self.is_maximized();
        // SAFETY: ventana de este hilo.
        let _ = unsafe { ShowWindow(self.hwnd, if maximized { SW_RESTORE } else { SW_MAXIMIZE }) };
        !maximized
    }

    pub fn is_maximized(&self) -> bool {
        // SAFETY: sin punteros.
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    #[allow(dead_code)] // los tests de la ventana
    pub fn is_visible(&self) -> bool {
        // SAFETY: sin punteros.
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Ejecuta JavaScript en la página (sin esperar el resultado).
    pub fn eval(&self, js: &str) {
        let Ok(webview) = self.shared.webview.try_borrow() else {
            return;
        };
        if let Some(webview) = webview.as_ref()
            && let Err(e) = webview.evaluate_script(js)
        {
            log::debug!("No se pudo enviar a la página: {e}");
        }
    }

    /// Dónde está la ventana, en píxeles físicos; maximizada o minimizada, dónde vuelve al
    /// restaurarla (lo que se guarda para la próxima vez).
    pub fn rect(&self) -> RECT {
        let _dpi = PerMonitorDpi::enter();
        let mut r = RECT::default();
        // SAFETY: estructuras con su tamaño, ventana de este hilo.
        unsafe {
            let mut wp = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            let restored = IsZoomed(self.hwnd).as_bool() || IsIconic(self.hwnd).as_bool();
            if restored && GetWindowPlacement(self.hwnd, &mut wp).is_ok() {
                // rcNormalPosition va en coordenadas del área de trabajo, no de la pantalla
                let mut mi = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let m = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
                let (dx, dy) = if GetMonitorInfoW(m, &mut mi).as_bool() {
                    (
                        mi.rcWork.left - mi.rcMonitor.left,
                        mi.rcWork.top - mi.rcMonitor.top,
                    )
                } else {
                    (0, 0)
                };
                let n = wp.rcNormalPosition;
                return RECT {
                    left: n.left + dx,
                    top: n.top + dy,
                    right: n.right + dx,
                    bottom: n.bottom + dy,
                };
            }
            let _ = GetWindowRect(self.hwnd, &mut r);
        }
        r
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for MainWindow {
    fn drop(&mut self) {
        // primero la página (cierra WebView2 y su ventana hija), luego la ventana
        let webview = self
            .shared
            .webview
            .try_borrow_mut()
            .ok()
            .and_then(|mut w| w.take());
        drop(webview);
        // SAFETY: ventana de este hilo; después de esto el procedimiento ya no ve `shared`.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn create_hwnd(rect: RECT) -> Result<HWND, String> {
    // SAFETY: clase y ventana de este proceso; `wndproc` vive lo que el proceso.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        // el icono del exe (recurso 1, ver build.rs); en los tests no hay
        let icon = |size: Option<(i32, i32)>| {
            let (cx, cy, flags) =
                size.map_or((0, 0, LR_DEFAULTSIZE), |(x, y)| (x, y, Default::default()));
            LoadImageW(
                Some(instance.into()),
                PCWSTR(std::ptr::without_provenance(1)),
                IMAGE_ICON,
                cx,
                cy,
                flags,
            )
            .map(|h| HICON(h.0))
            .unwrap_or_default()
        };
        let (r, g, b) = BACKGROUND;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hIcon: icon(None),
            hIconSm: icon(Some((
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
            ))),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(
                u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16,
            )),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            let e = windows::core::Error::from_thread();
            if e.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                return Err(format!("No se pudo registrar la ventana: {e}"));
            }
        }
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("Anotalo"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("No se pudo crear la ventana: {e}"))
    }
}

/// Esquinas redondeadas, modo oscuro y sombra (como winchrome.apply_chrome). En Windows 10 las
/// esquinas no existen: se ignora.
fn apply_chrome(hwnd: HWND) {
    let round = DWMWCP_ROUND;
    let dark = BOOL::from(true);
    let margins = MARGINS {
        cxLeftWidth: 0,
        cxRightWidth: 0,
        cyTopHeight: 1,
        cyBottomHeight: 0,
    };
    // SAFETY: valores que viven durante cada llamada, con su tamaño.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const round).cast(),
            size_of_val(&round) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

/// La ventana, para que wry cuelgue de ella la página.
struct Parent(HWND);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0.0 as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: la ventana existe mientras se construye la página (y después, hasta Drop).
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// Los datos de WebView2 (caché del motor): en `%LOCALAPPDATA%\Anotalo[-dev]\WebView2`, no en el
/// perfil móvil (`%APPDATA%`), como pide Microsoft.
fn webview_data_dir() -> PathBuf {
    let data = paths::data_dir(paths::profile());
    match (std::env::var_os("LOCALAPPDATA"), data.file_name()) {
        (Some(local), Some(name)) => PathBuf::from(local).join(name).join("WebView2"),
        _ => data.join("WebView2"),
    }
}

fn build_webview(hwnd: HWND, on_ipc: Box<dyn Fn(String)>) -> Result<WebView, String> {
    let mut context = WebContext::new(Some(webview_data_dir()));
    let (r, g, b) = BACKGROUND;
    WebViewBuilder::new_with_web_context(&mut context)
        .with_background_color((r, g, b, 255))
        .with_devtools(cfg!(debug_assertions))
        .with_default_context_menus(cfg!(debug_assertions))
        // sin F5, Ctrl+R, Ctrl+P ni Ctrl+F del navegador (como con pywebview)
        .with_browser_accelerator_keys(cfg!(debug_assertions))
        .with_https_scheme(true)
        .with_custom_protocol(PROTOCOL.into(), |_id, request| {
            serve(&request.uri().to_string())
        })
        .with_ipc_handler(move |request| on_ipc(request.into_body()))
        // solo la página de Anotalo: nada de navegar a otros sitios ni abrir ventanas
        .with_navigation_handler(|url| {
            let ok = url.starts_with(PAGE_ORIGIN) || url == "about:blank";
            if !ok {
                log::warn!("La página intentó ir a {url}: bloqueado");
            }
            ok
        })
        .with_new_window_req_handler(|url, _| {
            log::warn!("La página intentó abrir una ventana ({url}): bloqueado");
            NewWindowResponse::Deny
        })
        .build_as_child(&Parent(hwnd))
        .map_err(|e| format!("No se pudo crear la página (WebView2): {e}"))
}

/// CSS `app-region: drag` como barra de título nativa (arrastrar, encajar, doble clic para
/// maximizar). Con un WebView2 antiguo queda sin arrastre.
fn enable_drag_regions(webview: &WebView) {
    // SAFETY: llamadas COM en el hilo de la página.
    let result = unsafe {
        webview
            .webview()
            .Settings()
            .and_then(|s| s.cast::<ICoreWebView2Settings9>())
            .and_then(|s| s.SetIsNonClientRegionSupportEnabled(true))
    };
    if let Err(e) = result {
        log::warn!("WebView2 sin soporte de zonas de arrastre: {e}");
    }
}

/// `shared` de la ventana, si ya (o aún) lo tiene.
fn shared<'a>(hwnd: HWND) -> Option<&'a Shared> {
    // SAFETY: `GWLP_USERDATA` es 0 o apunta al `Shared` vivo de su `MainWindow` (ver Drop).
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared).as_ref() }
}

fn with_webview(hwnd: HWND, f: impl FnOnce(&WebView)) {
    if let Some(s) = shared(hwnd)
        && let Ok(webview) = s.webview.try_borrow()
        && let Some(webview) = webview.as_ref()
    {
        f(webview);
    }
}

/// La página ocupa la ventana menos el borde para redimensionar (nada si está maximizada).
fn fit_webview(hwnd: HWND) {
    let mut client = RECT::default();
    // SAFETY: ventana de este hilo.
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return;
    }
    // SAFETY: sin punteros.
    let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
    let border = if maximized {
        0
    } else {
        scaled(RESIZE_BORDER, dpi_of(hwnd))
    };
    let w = (client.right - client.left - 2 * border).max(0);
    let h = (client.bottom - client.top - 2 * border).max(0);
    with_webview(hwnd, |webview| {
        let bounds = Rect {
            position: PhysicalPosition::new(border, border).into(),
            size: PhysicalSize::new(w as u32, h as u32).into(),
        };
        if let Err(e) = webview.set_bounds(bounds) {
            log::debug!("No se pudo ajustar la página: {e}");
        }
    });
}

/// La coordenada con signo de la mitad baja de `v` (un `lParam` de ratón).
fn signed16(v: isize) -> i32 {
    i32::from(v as u16 as i16)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: mensajes de Windows para esta ventana, con sus punteros válidos.
    match catch_unwind(AssertUnwindSafe(|| unsafe { handle(hwnd, msg, wp, lp) })) {
        Ok(Some(r)) => return r,
        Ok(None) => {}
        Err(_) => log::error!("Ventana principal: error en el mensaje {msg:#06x}"),
    }
    // SAFETY: procedimiento por defecto de Windows.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// `None`: lo que Windows haga por defecto.
unsafe fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_NCCALCSIZE => {
            if wp.0 != 0 {
                // SAFETY: con wParam TRUE, lParam es un NCCALCSIZE_PARAMS.
                let p = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
                // SAFETY: sin punteros.
                let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
                p.rgrc[0] = frame::client_rect(
                    p.rgrc[0],
                    maximized,
                    frame::frame_inset(dpi_of(hwnd), metric_for_dpi),
                );
            }
            Some(LRESULT(0)) // sin marco: toda la ventana es contenido
        }
        WM_NCHITTEST => {
            let mut wr = RECT::default();
            // SAFETY: ventana de este hilo.
            unsafe { GetWindowRect(hwnd, &mut wr) }.ok()?;
            // SAFETY: sin punteros.
            let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
            let hit = frame::hit_test(
                signed16(lp.0) - wr.left,
                signed16(lp.0 >> 16) - wr.top,
                wr.right - wr.left,
                wr.bottom - wr.top,
                scaled(RESIZE_BORDER, dpi_of(hwnd)),
                maximized,
            );
            (hit != HTCLIENT).then_some(LRESULT(hit as isize))
        }
        WM_STYLECHANGING if wp.0 as u32 as i32 == GWL_STYLE.0 => {
            // SAFETY: con GWL_STYLE, lParam es un STYLESTRUCT.
            let ss = unsafe { &mut *(lp.0 as *mut STYLESTRUCT) };
            ss.styleNew = frame::keep_frame_style(ss.styleNew);
            None
        }
        WM_GETMINMAXINFO => {
            // SAFETY: lParam es un MINMAXINFO.
            let info = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            let dpi = dpi_of(hwnd);
            info.ptMinTrackSize = POINT {
                x: scaled(MIN_SIZE.0, dpi),
                y: scaled(MIN_SIZE.1, dpi),
            };
            Some(LRESULT(0))
        }
        WM_SIZE => {
            let kind = wp.0 as u32;
            if kind != SIZE_MINIMIZED {
                fit_webview(hwnd);
            }
            if let Some(s) = shared(hwnd) {
                let maximized = match kind {
                    SIZE_MAXIMIZED => Some(true),
                    SIZE_RESTORED => Some(false),
                    _ => None, // minimizar no cambia lo que muestra el botón
                };
                if let Some(m) = maximized
                    && s.maximized.replace(m) != m
                {
                    s.tell(WindowChange::Maximized(m));
                }
            }
            None
        }
        // wParam FALSE: se oculta; lParam 0: por ShowWindow (no por minimizar la ventana dueña)
        WM_SHOWWINDOW if wp.0 == 0 && lp.0 == 0 => {
            if let Some(s) = shared(hwnd) {
                s.tell(WindowChange::Hidden);
            }
            None
        }
        WM_MOVE => {
            // para que los desplegables y la accesibilidad de la página sepan dónde está
            with_webview(hwnd, |webview| {
                // SAFETY: llamada COM en el hilo de la página.
                let _ = unsafe { webview.controller().NotifyParentWindowPositionChanged() };
            });
            None
        }
        WM_DPICHANGED => {
            // SAFETY: lParam es el RECT que Windows propone para el nuevo DPI.
            let r = unsafe { &*(lp.0 as *const RECT) };
            // SAFETY: ventana de este hilo.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            fit_webview(hwnd); // el borde cambia con el DPI aunque el tamaño no
            Some(LRESULT(0))
        }
        WM_SETFOCUS => {
            with_webview(hwnd, |webview| {
                let _ = webview.focus();
            });
            None
        }
        WM_CLOSE => {
            // cerrar solo la oculta: la app sigue en la bandeja
            // SAFETY: ventana de este hilo.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            Some(LRESULT(0))
        }
        WM_NCDESTROY => {
            // SAFETY: la ventana se va; nada más debe ver `shared`.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
        TranslateMessage,
    };

    #[test]
    fn test_page_assets_served_with_content_type() {
        let (ty, page) = asset("/index.html").expect("index.html");
        assert_eq!(ty, "text/html; charset=utf-8");
        assert!(String::from_utf8_lossy(page).contains("bridge.js"));
        assert_eq!(asset("/").map(|a| a.1), Some(page), "/ es la página");
        assert!(asset("/no-existe.js").is_none());
        for (name, ty) in [
            ("app.css", "text/css; charset=utf-8"),
            ("app.js", "text/javascript; charset=utf-8"),
            ("fonts/Sora.woff2", "font/woff2"),
            ("fonts/Sora.TTF", "font/ttf"),
            ("icono.svg", "image/svg+xml"),
            ("icono.png", "image/png"),
            ("fonts/OFL-Sora.txt", "text/plain; charset=utf-8"),
            ("raro.bin", "application/octet-stream"),
        ] {
            assert_eq!(content_type(name), ty, "{name}");
        }
        // las rutas llegan codificadas
        assert_eq!(
            decode_path("/fonts/Instrument%20Sans.ttf"),
            "/fonts/Instrument Sans.ttf"
        );
        assert_eq!(decode_path("/a%C3%B1o.css"), "/año.css");
        assert_eq!(decode_path("/mal%2"), "/mal%2");

        let ok = serve(START_URL);
        assert_eq!(ok.status(), 200);
        assert_eq!(ok.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(ok.headers()["cache-control"], "no-store");
        assert_eq!(serve("anotalo://localhost/no-existe.js").status(), 404);
    }

    /// Mueve los mensajes de este hilo hasta que `done` o pase `limit`.
    fn pump_until(limit: Duration, done: impl Fn() -> bool) -> bool {
        let end = Instant::now() + limit;
        let mut msg = MSG::default();
        while Instant::now() < end {
            // SAFETY: cola de mensajes de este hilo.
            unsafe {
                MsgWaitForMultipleObjects(None, false, 50, QS_ALLINPUT);
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if done() {
                return true;
            }
        }
        false
    }

    #[test]
    #[ignore = "necesita WebView2 y una sesión de escritorio"]
    fn main_window_loads_the_page() {
        let got: Rc<RefCell<Vec<String>>> = Rc::default();
        let sink = got.clone();
        let win = MainWindow::create(
            Box::new(move |m| sink.borrow_mut().push(m)),
            true,
            initial_rect(None),
        )
        .expect("ventana principal");
        assert!(!win.is_visible(), "--minimized: empieza oculta");
        let ready = || {
            got.borrow().iter().any(|m| {
                serde_json::from_str::<serde_json::Value>(m).is_ok_and(|v| {
                    v["method"] == "log_js" && v["args"] == serde_json::json!(["ready"])
                })
            })
        };
        let loaded = pump_until(Duration::from_secs(10), ready);
        assert!(
            loaded,
            "la página no avisó en 10 s; llegó: {:?}",
            got.borrow()
        );
    }
}
