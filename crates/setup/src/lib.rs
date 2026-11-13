//! Instalar, actualizar y desinstalar Anotalo, y sustituir al Transcriptor de la era
//! de Inno (spec §2.3 y §5.8). Todo trabaja sobre un `Layout` (cada carpeta y clave del registro) y
//! un `System` (los otros procesos), para que las pruebas corran en carpetas temporales y bajo una
//! clave de prueba.

pub mod autostart;
pub mod cmdline;
pub mod install;
pub mod layout;
pub mod legacy;
pub mod system;
pub mod uninstall;
