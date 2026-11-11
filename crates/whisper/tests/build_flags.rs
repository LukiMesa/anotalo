//! whisper.cpp tiene que compilarse optimizado (/O2, sin asserts) y con excepciones de C++ (/EHsc):
//! cmake-rs sobrescribe las banderas de CMake y lo dejaba en /Od (revisión final de M0, C1).

#[test]
fn whisper_cpp_is_built_optimized_with_cpp_exceptions() {
    // target/<perfil>/deps/<test>.exe -> target/<perfil>/wcpp/build/CMakeCache.txt
    let profile = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let cache =
        std::fs::read_to_string(profile.join("wcpp").join("build").join("CMakeCache.txt")).unwrap();
    let value = |key: &str| {
        cache
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}:STRING=")))
            .unwrap_or("")
            .to_string()
    };
    let has = |flags: &str, f: &str| {
        flags
            .split_whitespace()
            .any(|x| x == format!("/{f}") || x == format!("-{f}"))
    };
    for lang in ["C", "CXX"] {
        let release = value(&format!("CMAKE_{lang}_FLAGS_RELEASE"));
        assert!(
            has(&release, "O2") && has(&release, "DNDEBUG"),
            "CMAKE_{lang}_FLAGS_RELEASE = {release:?}"
        );
    }
    let cxx = value("CMAKE_CXX_FLAGS");
    assert!(has(&cxx, "EHsc"), "CMAKE_CXX_FLAGS = {cxx:?}");
}
