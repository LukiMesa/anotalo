//! Core Audio de este PC (no hay dispositivos de audio en los runners de CI: ignorado).
//! `cargo test --release -p anotalo-winaudio -- --ignored --nocapture`
use anotalo_winaudio::coreaudio::{capture_keys, capture_snapshot, render_snapshot};

#[test]
#[ignore]
fn snapshot_lists_this_pcs_devices() {
    let render = render_snapshot().unwrap();
    println!(
        "Salidas (comm {:?}, console {:?}):",
        render.comm, render.console
    );
    for (id, name) in &render.names {
        println!("  {name}  [{id}]");
    }
    for s in &render.sessions {
        println!(
            "  sesión pid {} activa {} clave {:?} en {}",
            s.pid, s.active, s.key, s.endpoint_name
        );
    }
    assert!(render.names.values().any(|n| !n.is_empty()));
    assert!(render.console.is_some());
    assert!(render.names.contains_key(render.console.as_ref().unwrap()));

    let capture = capture_snapshot().unwrap();
    println!("Entradas (console {:?}):", capture.console);
    for name in capture.names.values() {
        println!("  {name}");
    }
    println!("Grabando ahora: {:?}", capture_keys().unwrap());
}
