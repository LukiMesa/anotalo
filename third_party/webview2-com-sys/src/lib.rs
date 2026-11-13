// Copia de Anotalo (ver README.md): las funciones del cargador de WebView2 no se enlazan con
// WebView2LoaderStatic.lib (el cargador precompilado de Microsoft); las define anotalo-winsys en
// crates/winsys/src/webview2_loader.rs con `#[unsafe(no_mangle)]`. Si no se enlazara, el enlace
// fallaría (LNK2019) en lugar de usar código de Microsoft.
#[allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    clippy::all
)]
pub mod Microsoft {
    pub mod Web {
        pub mod WebView2 {
            pub mod Win32 {
                mod windows_core {
                    macro_rules! link_webview2 {
                        ($library:literal $abi:literal fn $($function:tt)*) => (
                            extern $abi {
                                pub fn $($function)*;
                            }
                        )
                    }

                    pub(crate) use ::windows_core::*;
                    pub(crate) use link_webview2 as link;
                }

                include!("bindings.rs");
            }
        }
    }
}

pub mod declared_interfaces;
