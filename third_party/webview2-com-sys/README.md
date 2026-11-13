# webview2-com-sys
This crate implements unsafe Rust bindings for the [WebView2](https://aka.ms/webview2) COM APIs using the [Windows](https://github.com/microsoft/windows-rs) crate.

## Getting Started
This crate has a friendlier wrapper in [webview2-com](https://crates.io/crates/webview2-com).

## Copia de Anotalo

Copia de `webview2-com-sys` 0.39.1 de crates.io (repositorio https://github.com/wravery/webview2-rs,
commit `edc2caf8`, licencia MIT en `LICENSE`) que el `[patch.crates-io]` de la raíz usa en lugar
del original. El original enlaza en el exe `WebView2LoaderStatic.lib`, el cargador precompilado de
Microsoft, y la spec (§5.1) no admite código objeto precompilado de terceros en el exe.

- `src/bindings.rs` y `src/declared_interfaces.rs`: los originales, sin tocar.
- `src/lib.rs`: las cinco funciones del cargador (`CreateCoreWebView2Environment[WithOptions]`,
  `GetAvailableCoreWebView2BrowserVersionString[WithOptions]` y `CompareBrowserVersions`) se
  declaran sin `#[link]`: las define `anotalo-winsys` (`crates/winsys/src/webview2_loader.rs`).
  Si no se enlazara, el enlace fallaría (LNK2019); nunca vuelve el cargador de Microsoft.
- Sin `build.rs` ni las carpetas `x64`, `x86` y `arm64` (`WebView2LoaderStatic.lib`,
  `WebView2Loader.dll`). Sin los tests del original, que llamaban al cargador.

Para pasar a otra versión: copiar `src/bindings.rs` y `src/declared_interfaces.rs` de la nueva,
subir `version` en `Cargo.toml` y comprobar que las funciones del cargador siguen siendo esas cinco
(`grep -n "link!" src/bindings.rs`).
