//! Datos de versión del exe (SignPath exige el nombre del producto en los metadatos) e icono.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let icon = "../../calltx/ui/assets/icon.ico"; // hasta que llegue el icono de Anotalo (M3)
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set("ProductName", "Anotalo")
        .set("CompanyName", "LukiMesa")
        .set("FileDescription", "Anotalo")
        .set("ProductVersion", &version)
        .set("FileVersion", &version);
    res.compile()
        .expect("no se pudo compilar el recurso de versión");
}
