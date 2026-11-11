//! Una vulkan-1.dll puesta junto al exe no debe cargarse: solo la de System32 (revisión final, I2).
mod common;

#[test]
fn ignores_a_vulkan_dll_planted_next_to_the_exe() {
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let planted = exe_dir.join("vulkan-1.dll");
    std::fs::copy(r"C:\Windows\System32\version.dll", &planted).unwrap();
    let _ = anotalo_whisper::devices();
    let loaded = common::loaded_module_path("vulkan-1.dll");
    let _ = std::fs::remove_file(&planted);
    let system = std::path::Path::new(r"C:\Windows\System32\vulkan-1.dll").exists();
    match (system, loaded) {
        (true, Some(path)) => assert!(
            path.to_lowercase().contains(r"\windows\system32\"),
            "{path}"
        ),
        (false, None) => {}
        other => panic!("vulkan-1.dll cargada de donde no debía: {other:?}"),
    }
}
