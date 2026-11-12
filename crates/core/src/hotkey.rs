//! Formato de los atajos de teclado (el de la app de Python: `<ctrl>+<alt>+r`, `<ctrl>+<f9>`…) a
//! modificadores y tecla virtual de `RegisterHotKey`.

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

/// `"<ctrl>+<alt>+r"` → (modificadores con MOD_NOREPEAT, tecla virtual).
pub fn parse_hotkey(spec: &str) -> Result<(u32, u32), String> {
    let mut mods = MOD_NOREPEAT;
    let mut vk = None;
    for part in spec.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "<ctrl>" => mods |= MOD_CONTROL,
            "<alt>" => mods |= MOD_ALT,
            "<shift>" => mods |= MOD_SHIFT,
            "<cmd>" | "<win>" => mods |= MOD_WIN,
            p if p.len() == 1 && p.as_bytes()[0].is_ascii_alphanumeric() => {
                vk = Some(u32::from(p.as_bytes()[0].to_ascii_uppercase()));
            }
            p if p.starts_with("<f") && p.ends_with('>') => {
                match p[2..p.len() - 1].parse::<u32>() {
                    Ok(n @ 1..=24) => vk = Some(0x70 + n - 1),
                    _ => return Err(format!("Tecla no reconocida en el atajo: '{part}'")),
                }
            }
            _ => return Err(format!("Tecla no reconocida en el atajo: '{part}'")),
        }
    }
    match vk {
        Some(vk) => Ok((mods, vk)),
        None => Err(format!("El atajo '{spec}' no tiene tecla principal")),
    }
}

/// Como `parse_hotkey`, pero exige Ctrl, Alt o Win (Shift solo no basta).
pub fn validate_hotkey(spec: &str) -> Result<(u32, u32), String> {
    let (mods, vk) = parse_hotkey(spec)?;
    if mods & (MOD_CONTROL | MOD_ALT | MOD_WIN) == 0 {
        return Err("El atajo necesita Ctrl, Alt o Win".into());
    }
    Ok((mods, vk))
}
