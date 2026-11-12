//! Port de la parte de tests/test_hotkey.py que no toca Windows (el registro de los atajos está en
//! crates/anotalo/src/shell/hotkeys.rs).
use anotalo_core::hotkey::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, parse_hotkey, validate_hotkey,
};

#[test]
fn test_parse_default() {
    assert_eq!(
        parse_hotkey("<ctrl>+<alt>+r"),
        Ok((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, u32::from(b'R')))
    );
}

#[test]
fn test_parse_shift_digit() {
    assert_eq!(
        parse_hotkey("<ctrl>+<shift>+1"),
        Ok((MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, u32::from(b'1')))
    );
}

#[test]
fn test_parse_function_key() {
    assert_eq!(
        parse_hotkey("<ctrl>+<f9>"),
        Ok((MOD_CONTROL | MOD_NOREPEAT, 0x78))
    );
}

#[test]
fn test_parse_invalid() {
    assert_eq!(
        parse_hotkey("<ctrl>+<alt>"),
        Err("El atajo '<ctrl>+<alt>' no tiene tecla principal".to_string())
    );
    assert_eq!(
        parse_hotkey("<ctrl>+ñ"),
        Err("Tecla no reconocida en el atajo: 'ñ'".to_string())
    );
    assert!(parse_hotkey("<ctrl>+<f25>").is_err());
}

#[test]
fn test_validate_requires_modifier() {
    for bad in ["r", "<shift>+r"] {
        assert_eq!(
            validate_hotkey(bad),
            Err("El atajo necesita Ctrl, Alt o Win".to_string())
        );
    }
    assert_eq!(
        validate_hotkey("<ctrl>+<alt>+r").unwrap().1,
        u32::from(b'R')
    );
    assert_eq!(
        parse_hotkey(" <CTRL> + <Alt> + M "),
        parse_hotkey("<ctrl>+<alt>+m"),
        "como en Python, sin distinguir mayúsculas ni espacios"
    );
}
