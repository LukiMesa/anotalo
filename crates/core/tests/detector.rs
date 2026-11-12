//! Port de tests/test_detector.py y test_platforms.py::test_site_from_title, más el título de la
//! reunión (spec §2.4.2) y el periodo de gracia que vuelve a empezar.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anotalo_core::detector::{
    CallDetector, Detected, Probes, meeting_title, mic_keys, platforms_in_use,
};
use anotalo_core::routing::{PLATFORMS, platform_for_key, site_from_title};

const TEAMS: &str = "MSTeams_8wekyb3d8bbwe";
const FIREFOX: &str = "NonPackaged/C:#Program Files#Mozilla Firefox#firefox.exe";
const CHROME: &str = "NonPackaged/C:#Program Files#Google#Chrome#Application#chrome.exe";
const DISCORD: &str = "NonPackaged/C:#x#Discord#app-1.0.9#Discord.exe";
const WHATSAPP: &str = "5319275A.WhatsAppDesktop_cv1g1gvanyjgm";

#[derive(Default)]
struct State {
    t: f64,
    mic: HashSet<String>,
    /// clave de plataforma -> títulos de sus ventanas
    titles: HashMap<&'static str, Vec<String>>,
    disabled: HashSet<&'static str>,
    events: Vec<(String, String)>,
    detected: Vec<Detected>,
}

#[derive(Clone, Default)]
struct World(Arc<Mutex<State>>);

impl World {
    fn set_mic(&self, keys: &[&str]) {
        self.0.lock().unwrap().mic = keys.iter().map(|k| k.to_string()).collect();
    }
    fn set_title(&self, platform: &'static str, title: Option<&str>) {
        self.0
            .lock()
            .unwrap()
            .titles
            .insert(platform, title.into_iter().map(Into::into).collect());
    }
    fn events(&self) -> Vec<(String, String)> {
        self.0.lock().unwrap().events.clone()
    }
    fn detector(&self) -> CallDetector {
        let (a, b, c, d, e, f) = (
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
        );
        CallDetector::new(
            Box::new(move |det: Detected| {
                let mut s = a.0.lock().unwrap();
                s.events.push(("start".into(), det.label.clone()));
                s.detected.push(det);
            }),
            Box::new(move |det: Detected| {
                b.0.lock().unwrap().events.push(("stop".into(), det.label))
            }),
            Probes {
                mic: Box::new(move || c.0.lock().unwrap().mic.clone()),
                windows: Box::new(move |p| {
                    d.0.lock()
                        .unwrap()
                        .titles
                        .get(p.key)
                        .cloned()
                        .unwrap_or_default()
                }),
            },
            Box::new(move |p| !e.0.lock().unwrap().disabled.contains(p.key)),
            Box::new(move || f.0.lock().unwrap().t),
        )
    }
    /// Avanza el reloj de 2 en 2 s llamando a tick(), como el bucle real.
    fn advance(&self, d: &mut CallDetector, seconds: f64) {
        let end = self.0.lock().unwrap().t + seconds;
        while self.0.lock().unwrap().t < end {
            self.0.lock().unwrap().t += 2.0;
            d.tick();
        }
    }
}

fn ev(kind: &str, label: &str) -> (String, String) {
    (kind.into(), label.into())
}

#[test]
fn test_teams_start_after_hold() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[TEAMS]);
    d.tick();
    w.advance(&mut d, 2.0);
    assert!(w.events().is_empty());
    w.advance(&mut d, 4.0);
    assert_eq!(w.events(), vec![ev("start", "Teams")]);
}

#[test]
fn test_nothing_without_mic() {
    let w = World::default();
    let mut d = w.detector();
    w.advance(&mut d, 60.0);
    assert!(w.events().is_empty());
}

#[test]
fn test_browser_mic_starts_call_named_by_title() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[CHROME]);
    w.set_title("chrome", Some("Meet - abc-defg-hij - Google Chrome"));
    w.advance(&mut d, 8.0);
    assert_eq!(w.events(), vec![ev("start", "Meet")]);
}

#[test]
fn test_browser_without_known_title_uses_browser_name() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[FIREFOX]);
    w.advance(&mut d, 8.0);
    assert_eq!(w.events(), vec![ev("start", "Firefox")]);
}

#[test]
fn test_call_continues_when_title_gone() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[FIREFOX]);
    w.set_title("firefox", Some("Meet - abc-defg-hij — Mozilla Firefox"));
    w.advance(&mut d, 8.0);
    w.set_title("firefox", None);
    w.advance(&mut d, 600.0);
    assert_eq!(w.events(), vec![ev("start", "Meet")]);
}

#[test]
fn test_disabled_platform_ignored() {
    let w = World::default();
    let mut d = w.detector();
    w.0.lock().unwrap().disabled.insert("discord");
    w.set_mic(&[DISCORD]);
    w.advance(&mut d, 30.0);
    assert!(w.events().is_empty());
}

#[test]
fn test_whatsapp_voice_message_ignored_call_detected() {
    // WhatsApp usa el micro también al grabar una nota de voz: solo cuenta tras 30 s seguidos.
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[WHATSAPP]);
    w.advance(&mut d, 24.0); // nota de voz de 24 s
    w.set_mic(&[]);
    w.advance(&mut d, 20.0);
    assert!(w.events().is_empty());
    w.set_mic(&[WHATSAPP]); // llamada
    w.advance(&mut d, 28.0);
    assert!(w.events().is_empty());
    w.advance(&mut d, 4.0);
    assert_eq!(w.events(), vec![ev("start", "WhatsApp")]);
}

#[test]
fn test_second_platform_does_not_switch_active_call() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 8.0);
    w.set_mic(&[TEAMS, CHROME]);
    w.advance(&mut d, 30.0);
    assert_eq!(w.events(), vec![ev("start", "Teams")]);
    w.set_mic(&[CHROME]); // Teams suelta el micro: termina su llamada
    w.advance(&mut d, 14.0);
    assert_eq!(w.events()[..2], [ev("start", "Teams"), ev("stop", "Teams")]);
}

#[test]
fn test_stop_after_grace() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 8.0);
    w.set_mic(&[]);
    w.advance(&mut d, 6.0);
    assert!(!w.events().contains(&ev("stop", "Teams")));
    w.advance(&mut d, 8.0);
    assert_eq!(w.events().last(), Some(&ev("stop", "Teams")));
}

#[test]
fn test_brief_mic_release_no_stop() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 8.0);
    w.set_mic(&[]);
    w.advance(&mut d, 4.0);
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 30.0);
    assert_eq!(w.events(), vec![ev("start", "Teams")]);
}

#[test]
fn test_suspended_no_start() {
    let w = World::default();
    let mut d = w.detector();
    d.suspend();
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 20.0);
    assert!(w.events().is_empty());
    d.resume();
    w.advance(&mut d, 8.0);
    assert_eq!(w.events(), vec![ev("start", "Teams")]);
}

#[test]
fn test_manual_stop_dismisses_browser_until_release() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[FIREFOX]);
    w.advance(&mut d, 8.0);
    d.dismiss();
    w.advance(&mut d, 60.0);
    assert_eq!(w.events(), vec![ev("start", "Firefox")]);
    w.set_mic(&[]);
    w.advance(&mut d, 14.0);
    assert_eq!(w.events(), vec![ev("start", "Firefox")]); // sin aviso de fin
    w.set_mic(&[FIREFOX]);
    w.advance(&mut d, 8.0);
    assert_eq!(
        w.events(),
        vec![ev("start", "Firefox"), ev("start", "Firefox")]
    );
}

#[test]
fn test_detected_carries_platform_and_exe() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[DISCORD]);
    w.advance(&mut d, 8.0);
    let got = w.0.lock().unwrap().detected[0].clone();
    assert!(std::ptr::eq(
        got.platform,
        platform_for_key(DISCORD).unwrap()
    ));
    assert_eq!(got.exe.as_deref(), Some("discord.exe"));
}

#[test]
fn test_mic_in_use_parses_registry() {
    let entries: HashMap<String, (u64, u64)> = [
        (TEAMS, (134372538113007236, 0)),
        (
            "Microsoft.WindowsCamera_8wekyb3d8bbwe",
            (134372538113007236, 134372538423340618),
        ),
        (FIREFOX, (134372538113007236, 0)),
        ("NonPackaged/C:#never#used.exe", (0, 0)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let want: HashSet<String> = [TEAMS, FIREFOX].map(String::from).into();
    assert_eq!(mic_keys(Ok(entries), Ok(HashSet::new())), want);
}

#[test]
fn test_mic_in_use_also_trusts_windows_audio_sessions() {
    // En un PC el registro de privacidad no anotaba a Teams aunque grababa del micro.
    let seen = mic_keys(
        Ok(HashMap::new()),
        Ok(["NonPackaged/#ms-teams.exe".to_string()].into()),
    );
    assert_eq!(platforms_in_use(&seen), ["teams"].into());
    let both = mic_keys(
        Ok([(TEAMS.to_string(), (1, 0))].into()),
        Ok([TEAMS.to_string()].into()),
    );
    assert_eq!(both, [TEAMS.to_string()].into());
}

#[test]
fn test_mic_in_use_survives_either_source_failing() {
    assert_eq!(
        mic_keys(Err("sin acceso".into()), Ok([TEAMS.to_string()].into())),
        [TEAMS.to_string()].into()
    );
    assert_eq!(
        mic_keys(
            Ok([(FIREFOX.to_string(), (1, 0))].into()),
            Err("sin acceso".into())
        ),
        [FIREFOX.to_string()].into()
    );
}

#[test]
fn test_dismissed_browser_does_not_hide_other_calls() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[FIREFOX]);
    w.advance(&mut d, 8.0);
    d.dismiss(); // la web del navegador no era una llamada: parada a mano
    w.set_mic(&[FIREFOX, TEAMS]); // la pestaña sigue con el micro y entra una llamada de Teams
    w.advance(&mut d, 8.0);
    assert_eq!(
        w.events(),
        vec![ev("start", "Firefox"), ev("start", "Teams")]
    );
}

#[test]
fn test_dismiss_in_use_blocks_restart_until_release() {
    let w = World::default();
    let mut d = w.detector();
    d.suspend();
    w.set_mic(&[CHROME]);
    w.advance(&mut d, 20.0);
    d.dismiss_in_use();
    d.resume();
    w.advance(&mut d, 30.0);
    assert!(w.events().is_empty());
    w.set_mic(&[]);
    w.advance(&mut d, 14.0);
    w.set_mic(&[CHROME]);
    w.advance(&mut d, 8.0);
    assert_eq!(w.events(), vec![ev("start", "Chrome")]);
}

#[test]
fn test_dismiss_without_active_call_is_harmless() {
    let w = World::default();
    let mut d = w.detector();
    d.dismiss();
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 8.0);
    w.set_mic(&[]);
    w.advance(&mut d, 14.0);
    assert_eq!(w.events(), vec![ev("start", "Teams"), ev("stop", "Teams")]);
}

#[test]
fn test_site_from_title() {
    assert_eq!(
        site_from_title("Meet - abc-defg-hij - Google Chrome"),
        Some("Meet")
    );
    assert_eq!(
        site_from_title("Google Meet — Mozilla Firefox"),
        Some("Meet")
    );
    assert_eq!(
        site_from_title("Reunión | Microsoft Teams — Mozilla Firefox"),
        Some("Teams")
    );
    assert_eq!(site_from_title("Zoom Meeting - Brave"), Some("Zoom"));
    assert_eq!(
        site_from_title("(3) WhatsApp - Google Chrome"),
        Some("WhatsApp")
    );
    assert_eq!(site_from_title("YouTube - Google Chrome"), None);
    assert_eq!(
        site_from_title("Meeting notes – Google Docs — Mozilla Firefox"),
        None
    );
    assert_eq!(
        site_from_title("Meetup: Python Madrid — Mozilla Firefox"),
        None
    );
}

#[test]
fn test_release_resets_when_mic_returns_at_9s() {
    // Silenciar y reactivar el micro a mitad de llamada: la gracia de 10 s vuelve a empezar.
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 8.0);
    w.set_mic(&[]);
    w.advance(&mut d, 8.0); // 8 s sin micro
    w.set_mic(&[TEAMS]);
    w.advance(&mut d, 2.0); // vuelve un momento
    w.set_mic(&[]);
    w.advance(&mut d, 8.0); // otros 8 s: no son 10 seguidos
    assert_eq!(w.events(), vec![ev("start", "Teams")]);
}

#[test]
fn test_detected_carries_meeting_title() {
    let w = World::default();
    let mut d = w.detector();
    w.set_mic(&[TEAMS]);
    w.set_title("teams", Some("Reunión con Acme | Microsoft Teams"));
    w.advance(&mut d, 8.0);
    let got = w.0.lock().unwrap().detected[0].clone();
    assert_eq!(got.label, "Teams");
    assert_eq!(got.title.as_deref(), Some("Reunión con Acme"));
}

fn platform(key: &str) -> &'static anotalo_core::routing::Platform {
    PLATFORMS.iter().find(|p| p.key == key).unwrap()
}

#[test]
fn test_meeting_title_table() {
    let t = |key: &str, titles: &[&str]| {
        meeting_title(
            platform(key),
            &titles.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    };
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        t("teams", &["Reunión con Acme | Microsoft Teams"]),
        some("Reunión con Acme")
    );
    assert_eq!(
        t("teams", &["Revisión semanal | Contoso | Microsoft Teams"]),
        some("Revisión semanal")
    );
    assert_eq!(
        t(
            "teams",
            &[
                "Chat | Microsoft Teams",
                "Calendario | Contoso | Microsoft Teams",
                "Microsoft Teams"
            ]
        ),
        None
    );
    assert_eq!(
        t("teams", &["Weekly sync with Acme | Microsoft Teams"]),
        some("Weekly sync with Acme")
    );
    assert_eq!(
        t(
            "zoom",
            &["Zoom Meeting", "Reunión de Zoom", "Zoom Workplace"]
        ),
        None
    );
    assert_eq!(t("chrome", &["Meet - abc-defg-hij - Google Chrome"]), None);
    assert_eq!(
        t("chrome", &["Meet - Revisión con Acme - Google Chrome"]),
        some("Revisión con Acme")
    );
    assert_eq!(
        t("firefox", &["Meet: Revisión con Acme — Mozilla Firefox"]),
        some("Revisión con Acme")
    );
    assert_eq!(
        t(
            "firefox",
            &["Reunión con Acme | Microsoft Teams — Mozilla Firefox"]
        ),
        some("Reunión con Acme")
    );
    assert_eq!(t("chrome", &["YouTube - Google Chrome"]), None);
    assert_eq!(t("whatsapp", &["WhatsApp"]), None);
}
