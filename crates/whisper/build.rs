//! Compila whisper.cpp (submódulo third_party/whisper.cpp) desde el código fuente:
//! estático, Vulkan + CPU, CRT estático, sin OpenMP, y con `vulkan-1.dll` de carga diferida
//! protegida (sin Vulkan en el PC se usa la CPU en lugar de cerrarse). Spec §5.1–5.2.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// Cambia este valor si cambia el parche: obliga a copiar y parchear de nuevo.
const PATCH_STAMP: &str = "anotalo-vulkan-guard-3";

const SKIP_TOP: &[&str] = &[".git", "models", "samples", "examples"];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("raíz del repositorio")
        .to_path_buf();
    let upstream = root.join("third_party").join("whisper.cpp");
    let vulkan_sdk = env::var("VULKAN_SDK")
        .unwrap_or_else(|_| panic!("Falta VULKAN_SDK (instala el Vulkan SDK 1.4.363.0)"));

    // 1. Carpeta corta: el generador de shaders de ggml falla con rutas de más de 260 caracteres.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("target/<perfil>")
        .to_path_buf();
    let wcpp = profile_dir.join("wcpp");
    let src = wcpp.join("src");

    // 2–3. Copia del código y parche, solo si cambió el commit o el parche.
    let commit = git_head(&upstream);
    let stamp = format!("{commit} {PATCH_STAMP} {SKIP_TOP:?}");
    let stamp_file = wcpp.join("source.stamp");
    if fs::read_to_string(&stamp_file).ok().as_deref() != Some(stamp.as_str()) {
        if src.exists() {
            fs::remove_dir_all(&src).expect("no se pudo limpiar la copia de whisper.cpp");
        }
        copy_dir(&upstream, &src, true);
        patch_vulkan_guard(
            &src.join("ggml")
                .join("src")
                .join("ggml-vulkan")
                .join("ggml-vulkan.cpp"),
        );
        fs::create_dir_all(&wcpp).unwrap();
        fs::write(&stamp_file, &stamp).unwrap();
    }

    // 4. CMake (el de Visual Studio si no hay uno en el PATH).
    ensure_cmake();
    cmake::Config::new(&src)
        .out_dir(&wcpp)
        .profile("Release")
        .static_crt(true)
        .define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")
        .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("GGML_VULKAN", "ON")
        .define("GGML_OPENMP", "OFF")
        .define("GGML_NATIVE", "OFF")
        .define("GGML_AVX", "ON")
        .define("GGML_AVX2", "ON")
        .define("GGML_FMA", "ON")
        .define("GGML_F16C", "ON")
        .define("GGML_AVX512", "OFF")
        .define("GGML_CCACHE", "OFF")
        .define("WHISPER_BUILD_EXAMPLES", "OFF")
        .define("WHISPER_BUILD_TESTS", "OFF")
        .build();

    // 5. Enlace: todo estático salvo vulkan-1.dll (de carga diferida en el exe y en los tests).
    println!(
        "cargo:rustc-link-search=native={}",
        wcpp.join("lib").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        Path::new(&vulkan_sdk).join("Lib").display()
    );
    for lib in ["whisper", "ggml", "ggml-base", "ggml-cpu", "ggml-vulkan"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }
    println!("cargo:rustc-link-lib=dylib=vulkan-1");
    println!("cargo:rustc-link-lib=delayimp");
    println!("cargo:rustc-link-lib=advapi32"); // ggml-cpu lee el nombre del procesador del registro
    println!("cargo:rustc-link-arg-tests=/DELAYLOAD:vulkan-1.dll");

    // 6. El puente en C.
    cc::Build::new()
        .file(manifest.join("shim").join("anotalo_whisper.c"))
        .include(src.join("include"))
        .include(src.join("ggml").join("include"))
        .static_crt(true)
        .compile("anotalo_whisper_shim");

    // 7. Cuándo volver a compilar.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shim/anotalo_whisper.c");
    println!("cargo:rerun-if-changed=shim/anotalo_whisper.h");
    let module_head = root
        .join(".git")
        .join("modules")
        .join("third_party")
        .join("whisper.cpp")
        .join("HEAD");
    if module_head.exists() {
        println!("cargo:rerun-if-changed={}", module_head.display());
    }
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
}

fn git_head(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("hace falta git para leer el commit de whisper.cpp");
    assert!(
        out.status.success(),
        "falta el submódulo: git submodule update --init"
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn copy_dir(from: &Path, to: &Path, top: bool) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if top && SKIP_TOP.iter().any(|s| name == *s) {
            continue;
        }
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&path, &to.join(&name), false);
        } else {
            fs::copy(&path, to.join(&name)).unwrap();
        }
    }
}

/// Sin `vulkan-1.dll` (PC sin driver de GPU, máquina virtual) ggml no debe tocar Vulkan:
/// como la DLL es de carga diferida, la primera llamada cerraría el programa.
fn patch_vulkan_guard(file: &Path) {
    let code = fs::read_to_string(file).expect("falta ggml-vulkan.cpp");
    // Git en Windows suele sacar el submódulo con CRLF (core.autocrlf): se respeta el del archivo.
    let eol = if code.contains("\r\n") { "\r\n" } else { "\n" };
    let reg = "ggml_backend_reg_t ggml_backend_vk_reg() {";
    let try_init =
        "    try {\n        ggml_vk_instance_init();\n        return &reg;".replace('\n', eol);
    let try_init = try_init.as_str();
    for anchor in [reg, try_init] {
        assert_eq!(
            code.matches(anchor).count(),
            1,
            "el parche de Vulkan no encuentra su ancla en ggml-vulkan.cpp (¿cambió whisper.cpp?): {anchor:?}"
        );
    }
    // windows.h ya llega a este archivo por otras cabeceras: incluirlo otra vez no cambia nada,
    // y declarar LoadLibraryW a mano choca con su declaración oficial.
    let guard = r#"#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#endif
// Anotalo: vulkan-1.dll is delay-loaded. Without it (no GPU driver), stay on the CPU instead of crashing.
static bool anotalo_vulkan_loader_available() {
#ifdef _WIN32
    if (GetEnvironmentVariableW(L"ANOTALO_NO_VULKAN", nullptr, 0) > 0) { return false; }
    return LoadLibraryW(L"vulkan-1.dll") != nullptr;
#else
    return true;
#endif
}

"#
    .replace('\n', eol);
    let code = code.replacen(reg, &format!("{guard}{reg}"), 1).replacen(
        try_init,
        &format!(
            "    if (!anotalo_vulkan_loader_available()) {{ return nullptr; }}{eol}{try_init}"
        ),
        1,
    );
    fs::write(file, code).unwrap();
}

/// cmake-rs busca `cmake` en el PATH o en la variable CMAKE; con solo las Build Tools de
/// Visual Studio, CMake está dentro de su carpeta.
fn ensure_cmake() {
    if env::var_os("CMAKE").is_some() || Command::new("cmake").arg("--version").output().is_ok() {
        return;
    }
    let vswhere =
        Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    let found = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-find",
            r"Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe",
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(str::trim).map(str::to_string))
        .filter(|s| !s.is_empty());
    match found {
        // SAFETY: el script de compilación es de un solo hilo.
        Some(path) => unsafe { env::set_var("CMAKE", path) },
        None => panic!("Falta CMake: instala las Build Tools de Visual Studio con C++ o CMake"),
    }
}
