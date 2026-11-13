//! Lo visible de la app en Windows: la ventana de mensajes, los atajos, la bandeja y el indicador.

#[allow(dead_code)] // M3: la usa la ventana principal (tarea 8 la une a la app)
pub mod frame;
pub mod hotkeys;
pub mod indicator;
#[allow(dead_code)] // M3: la tarea 8 la une a la app de la bandeja
pub mod main_window;
pub mod pill;
pub mod tray;
pub mod window;
