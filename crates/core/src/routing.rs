//! Qué dispositivo grabar: el que usa de verdad la app de la llamada (port de
//! calltx/audio/routing.py y de la tabla de calltx/platforms.py).
//!
//! Windows dice qué programas tienen sonido abierto en cada dispositivo (sesiones de audio). Con
//! auriculares Bluetooth, durante una llamada la app suena en el dispositivo "manos libres", que no
//! suele ser el predeterminado: hay que seguir a la app, no al predeterminado.

/// De dónde sale la voz de los demás.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audio {
    /// Solo el audio del proceso de la app (y sus hijos).
    Process,
    /// Todo el dispositivo de salida donde suena la app.
    Device,
}

/// Una plataforma de llamadas que se reconoce sola.
#[derive(Debug, PartialEq)]
pub struct Platform {
    pub key: &'static str,
    /// Nombre que se ve en la grabación ("Llamada de Zoom").
    pub name: &'static str,
    /// Ajuste que la activa.
    pub setting: &'static str,
    /// Procesos, en minúsculas.
    pub exes: &'static [&'static str],
    /// Prefijos de la app empaquetada en el registro, en minúsculas.
    pub packaged: &'static [&'static str],
    pub audio: Audio,
    /// Segundos de micrófono antes de contar como llamada (None = el general).
    pub start_hold_s: Option<f64>,
    pub browser: bool,
}

const fn app(
    key: &'static str,
    name: &'static str,
    setting: &'static str,
    exes: &'static [&'static str],
    packaged: &'static [&'static str],
) -> Platform {
    Platform {
        key,
        name,
        setting,
        exes,
        packaged,
        audio: Audio::Process,
        start_hold_s: None,
        browser: false,
    }
}

const fn browser(key: &'static str, name: &'static str, exes: &'static [&'static str]) -> Platform {
    Platform {
        browser: true,
        ..app(key, name, "detect_browsers", exes, &[])
    }
}

pub const PLATFORMS: &[Platform] = &[
    // Capturar solo el proceso de Teams da silencio en algunos PCs (fallo conocido de Windows): se
    // graba el dispositivo donde suena Teams.
    Platform {
        audio: Audio::Device,
        ..app(
            "teams",
            "Teams",
            "detect_teams",
            &["ms-teams.exe", "teams.exe"],
            &["msteams_", "microsoftteams_"],
        )
    },
    app("zoom", "Zoom", "detect_zoom", &["zoom.exe"], &[]),
    // WhatsApp usa el micro también al grabar notas de voz y su ventana no cambia: solo cuenta tras
    // 30 s seguidos. Suena desde procesos de WebView2 fuera de su árbol: se graba el dispositivo.
    Platform {
        audio: Audio::Device,
        start_hold_s: Some(30.0),
        ..app(
            "whatsapp",
            "WhatsApp",
            "detect_whatsapp",
            &["whatsapp.root.exe", "whatsapp.exe"],
            &["5319275a.whatsappdesktop_"],
        )
    },
    app(
        "slack",
        "Slack",
        "detect_slack",
        &["slack.exe"],
        &["91750d7e.slack_"],
    ),
    app(
        "discord",
        "Discord",
        "detect_discord",
        &["discord.exe"],
        &[],
    ),
    browser("firefox", "Firefox", &["firefox.exe"]),
    browser("chrome", "Chrome", &["chrome.exe"]),
    browser("brave", "Brave", &["brave.exe"]),
    browser("edge", "Edge", &["msedge.exe"]),
    browser("opera", "Opera", &["opera.exe"]),
    browser("vivaldi", "Vivaldi", &["vivaldi.exe"]),
];

/// Plataforma de una entrada del registro del micrófono (empaquetada o "NonPackaged/…#app.exe").
pub fn platform_for_key(key: &str) -> Option<&'static Platform> {
    let k = key.to_lowercase();
    if k.starts_with("nonpackaged/") {
        let exe = k.rsplit('#').next().unwrap_or_default();
        return PLATFORMS.iter().find(|p| p.exes.contains(&exe));
    }
    PLATFORMS
        .iter()
        .find(|p| p.packaged.iter().any(|pre| k.starts_with(pre)))
}

/// Un programa con sonido abierto en un dispositivo.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub endpoint_id: String,
    pub endpoint_name: String,
    pub pid: u32,
    /// Sonando o grabando ahora mismo.
    pub active: bool,
    /// Como en el registro del micrófono: el paquete o "NonPackaged/#app.exe".
    pub key: Option<String>,
}

/// Clave al estilo del registro del micrófono, para reconocer la plataforma.
pub fn process_key(exe: Option<&str>, package: Option<&str>) -> Option<String> {
    match (package, exe) {
        (Some(p), _) if !p.is_empty() => Some(p.to_string()),
        (_, Some(e)) if !e.is_empty() => Some(format!("NonPackaged/#{}", e.to_lowercase())),
        _ => None,
    }
}

/// La plataforma de llamadas de esta sesión, si es la buscada (o cualquiera, sin `platform_key`).
fn call_app(info: &SessionInfo, platform_key: Option<&str>) -> Option<&'static Platform> {
    let p = platform_for_key(info.key.as_deref()?)?;
    platform_key.is_none_or(|k| p.key == k).then_some(p)
}

/// El primero de `preferred` que esté en `endpoints`; si no, el primero de `endpoints` (no vacío).
fn prefer<'a>(endpoints: &[&'a str], preferred: &[Option<&str>]) -> &'a str {
    preferred
        .iter()
        .flatten()
        .find_map(|p| endpoints.iter().find(|e| *e == p).copied())
        .unwrap_or(endpoints[0])
}

/// Dispositivos sin repetir, en el orden en que aparecen.
fn unique<'a>(sessions: &[&'a SessionInfo]) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for s in sessions {
        if !out.contains(&s.endpoint_id.as_str()) {
            out.push(&s.endpoint_id);
        }
    }
    out
}

/// Dispositivo de salida donde escuchar a "los demás" y por qué. `platform_key`: la plataforma de
/// la llamada, o None (grabación a mano: cualquier app de llamadas). `comm` / `console`: los
/// predeterminados de comunicaciones y general.
pub fn pick_render(
    sessions: &[SessionInfo],
    platform_key: Option<&str>,
    comm: Option<&str>,
    console: Option<&str>,
) -> (Option<String>, String) {
    let active: Vec<&SessionInfo> = sessions.iter().filter(|s| s.active).collect();
    let hits: Vec<&SessionInfo> = active
        .iter()
        .copied()
        .filter(|s| call_app(s, platform_key).is_some())
        .collect();
    if !hits.is_empty() {
        let ep = prefer(&unique(&hits), &[comm, console]);
        let name = hits
            .iter()
            .find(|s| s.endpoint_id == ep)
            .and_then(|s| call_app(s, platform_key))
            .map_or("", |p| p.name);
        return (Some(ep.to_string()), format!("donde suena {name}"));
    }
    let sounding = unique(&active);
    if !sounding.is_empty() {
        let ep = prefer(&sounding, &[comm, console]);
        let why = if comm == Some(ep) || console == Some(ep) {
            "predeterminado con sonido"
        } else {
            "con sonido"
        };
        return (Some(ep.to_string()), why.into());
    }
    (
        console.or(comm).map(str::to_string),
        "predeterminado".into(),
    )
}

/// Micrófono para "Yo": el que usa la app de la llamada; si no se sabe, el predeterminado.
pub fn pick_capture(
    sessions: &[SessionInfo],
    platform_key: Option<&str>,
    console: Option<&str>,
) -> (Option<String>, String) {
    let hits: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| s.active && call_app(s, platform_key).is_some())
        .collect();
    match hits.first().and_then(|s| call_app(s, platform_key)) {
        Some(app) => {
            let ep = prefer(&unique(&hits), &[console]);
            (Some(ep.to_string()), format!("el que usa {}", app.name))
        }
        None => (console.map(str::to_string), "predeterminado".into()),
    }
}

/// Plataforma reconocible en el título de una ventana del navegador (p. ej. "Meet"), o None.
pub fn site_from_title(title: &str) -> Option<&'static str> {
    use std::sync::OnceLock;
    static SITES: OnceLock<Vec<(&'static str, regex::Regex)>> = OnceLock::new();
    let sites = SITES.get_or_init(|| {
        [
            ("Meet", r"^Meet\s*[-–—:]|Google Meet"),
            ("Teams", r"Microsoft Teams"),
            ("Zoom", r"\bZoom\b"),
            ("WhatsApp", r"\bWhatsApp\b"),
            ("Discord", r"\bDiscord\b"),
            ("Slack", r"\bSlack\b"),
        ]
        .into_iter()
        .map(|(name, rx)| (name, regex::Regex::new(rx).expect("regex válida")))
        .collect()
    });
    sites
        .iter()
        .find(|(_, rx)| rx.is_match(title))
        .map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAMS: &str = "MSTeams_8wekyb3d8bbwe";
    const CHROME: &str = "NonPackaged/#chrome.exe";
    const SPOTIFY: &str = "NonPackaged/#spotify.exe";

    fn s(endpoint: &str, key: &str) -> SessionInfo {
        SessionInfo {
            endpoint_id: endpoint.into(),
            endpoint_name: endpoint.to_uppercase(),
            pid: 1,
            active: true,
            key: Some(key.into()),
        }
    }

    fn idle(endpoint: &str, key: &str) -> SessionInfo {
        SessionInfo {
            active: false,
            ..s(endpoint, key)
        }
    }

    fn ep(r: (Option<String>, String)) -> Option<String> {
        r.0
    }

    #[test]
    fn process_key_matches_registry_style() {
        assert_eq!(
            process_key(Some("ms-teams.exe"), Some(TEAMS)).as_deref(),
            Some(TEAMS)
        );
        assert_eq!(
            process_key(Some("Chrome.exe"), None).as_deref(),
            Some(CHROME)
        );
        assert_eq!(process_key(None, None), None);
    }

    #[test]
    fn render_follows_the_call_app_to_its_device() {
        // Bluetooth en manos libres: Teams suena ahí aunque el predeterminado sean los altavoces
        let sessions = [s("spk", SPOTIFY), s("bt-hf", TEAMS), idle("spk", TEAMS)];
        let (ep, why) = pick_render(&sessions, Some("teams"), Some("spk"), Some("spk"));
        assert_eq!(ep.as_deref(), Some("bt-hf"));
        assert!(why.contains("Teams"), "{why}");
    }

    #[test]
    fn render_prefers_default_devices_when_the_app_plays_on_several() {
        let sessions = [s("a", TEAMS), s("b", TEAMS)];
        assert_eq!(
            ep(pick_render(&sessions, Some("teams"), Some("b"), Some("a"))).as_deref(),
            Some("b")
        );
        assert_eq!(
            ep(pick_render(&sessions, Some("teams"), None, Some("a"))).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn manual_call_picks_any_call_app() {
        let sessions = [s("spk", SPOTIFY), s("bt", CHROME)];
        let (ep, why) = pick_render(&sessions, None, Some("spk"), Some("spk"));
        assert_eq!(ep.as_deref(), Some("bt"));
        assert!(why.contains("Chrome"), "{why}");
    }

    #[test]
    fn render_falls_back_to_devices_with_sound_then_defaults() {
        // sin app de llamadas sonando: el de comunicaciones si suena, luego el predeterminado,
        // luego cualquiera
        let r = |sessions: &[SessionInfo], key, comm, console| {
            ep(pick_render(sessions, key, comm, console))
        };
        assert_eq!(
            r(
                &[s("comm", SPOTIFY), s("con", SPOTIFY)],
                Some("teams"),
                Some("comm"),
                Some("con")
            )
            .as_deref(),
            Some("comm")
        );
        assert_eq!(
            r(
                &[s("con", SPOTIFY)],
                Some("teams"),
                Some("comm"),
                Some("con")
            )
            .as_deref(),
            Some("con")
        );
        assert_eq!(
            r(&[s("bt", SPOTIFY)], None, Some("comm"), Some("con")).as_deref(),
            Some("bt")
        );
        assert_eq!(
            r(&[], None, Some("comm"), Some("con")).as_deref(),
            Some("con")
        );
        assert_eq!(
            r(&[idle("x", SPOTIFY)], None, Some("comm"), None).as_deref(),
            Some("comm")
        );
        assert_eq!(r(&[], None, None, None), None);
    }

    #[test]
    fn capture_uses_the_mic_of_the_call_app() {
        let sessions = [
            s("laptop-mic", "NonPackaged/#python.exe"),
            s("bt-mic", TEAMS),
        ];
        let c = |key, console| ep(pick_capture(&sessions, key, console));
        assert_eq!(
            c(Some("teams"), Some("laptop-mic")).as_deref(),
            Some("bt-mic")
        );
        assert_eq!(c(None, Some("laptop-mic")).as_deref(), Some("bt-mic"));
        assert_eq!(
            c(Some("zoom"), Some("laptop-mic")).as_deref(),
            Some("laptop-mic")
        );
        assert_eq!(ep(pick_capture(&[], Some("teams"), None)), None);
    }

    #[test]
    fn platform_for_registry_keys() {
        let k = |key: &str| platform_for_key(key).map(|p| p.key);
        assert_eq!(k("MSTeams_8wekyb3d8bbwe"), Some("teams"));
        assert_eq!(k("MicrosoftTeams_8wekyb3d8bbwe"), Some("teams")); // Teams personal (antiguo)
        assert_eq!(k("NonPackaged/#ms-teams.exe"), Some("teams")); // visto por el audio de Windows
        assert_eq!(
            k("5319275A.WhatsAppDesktop_cv1g1gvanyjgm"),
            Some("whatsapp")
        );
        assert_eq!(
            k("NonPackaged/C:#Users#x#AppData#Roaming#Zoom#bin#Zoom.exe"),
            Some("zoom")
        );
        assert_eq!(
            platform_for_key(
                "NonPackaged/C:#Program Files#BraveSoftware#Brave-Browser#Application#brave.exe"
            )
            .unwrap()
            .name,
            "Brave"
        );
        assert_eq!(
            platform_for_key("NonPackaged/C:#x#Discord#app-1.0.9#Discord.exe")
                .unwrap()
                .setting,
            "detect_discord"
        );
        assert_eq!(
            platform_for_key("NonPackaged/C:#Program Files#Mozilla Firefox#firefox.exe")
                .unwrap()
                .setting,
            "detect_browsers"
        );
        for other in [
            "NonPackaged/C:#x#dictado.exe",
            "NonPackaged/C:#x#Transcriptor.exe",
            "NonPackaged/C:#x#Anotalo.exe",
            "NonPackaged/C:#x#python.exe",
            "NonPackaged/C:#x#game.exe",
            "Microsoft.WindowsSoundRecorder_8wekyb3d8bbwe",
        ] {
            assert_eq!(k(other), None, "{other}");
        }
    }

    #[test]
    fn teams_and_whatsapp_record_their_output_device() {
        let audio = |key: &str| PLATFORMS.iter().find(|p| p.key == key).unwrap().audio;
        assert_eq!(audio("teams"), Audio::Device);
        assert_eq!(audio("whatsapp"), Audio::Device);
        assert_eq!(audio("zoom"), Audio::Process);
        assert_eq!(audio("chrome"), Audio::Process);
        assert!(
            PLATFORMS
                .iter()
                .find(|p| p.key == "chrome")
                .unwrap()
                .browser
        );
    }
}
