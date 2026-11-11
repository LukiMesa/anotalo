from calltx.detector import CallDetector, mic_in_use
from calltx.platforms import platform_for_key

TEAMS = "MSTeams_8wekyb3d8bbwe"
FIREFOX = "NonPackaged/C:#Program Files#Mozilla Firefox#firefox.exe"
CHROME = "NonPackaged/C:#Program Files#Google#Chrome#Application#chrome.exe"
DISCORD = "NonPackaged/C:#x#Discord#app-1.0.9#Discord.exe"
WHATSAPP = "5319275A.WhatsAppDesktop_cv1g1gvanyjgm"


class World:
    def __init__(self):
        self.t = 0.0
        self.mic: set[str] = set()
        self.titles: dict[str, str | None] = {}  # clave de plataforma -> sitio reconocido
        self.disabled: set[str] = set()
        self.events: list[tuple[str, str]] = []  # (start|stop, etiqueta)

    def detector(self):
        return CallDetector(
            on_start=lambda d: self.events.append(("start", d.label)),
            on_stop=lambda d: self.events.append(("stop", d.label)),
            probe_mic=lambda: set(self.mic),
            probe_title=lambda p: self.titles.get(p.key),
            enabled=lambda p: p.key not in self.disabled,
            clock=lambda: self.t,
        )

    def advance(self, d, seconds, step=2.0):
        end = self.t + seconds
        while self.t < end:
            self.t += step
            d.tick()


def test_teams_start_after_hold():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    d.tick()
    w.advance(d, 2)
    assert w.events == []
    w.advance(d, 4)
    assert w.events == [("start", "Teams")]


def test_nothing_without_mic():
    w = World()
    d = w.detector()
    w.advance(d, 60)
    assert w.events == []


def test_browser_mic_starts_call_named_by_title():
    w = World()
    d = w.detector()
    w.mic = {CHROME}
    w.titles["chrome"] = "Meet"
    w.advance(d, 8)
    assert w.events == [("start", "Meet")]


def test_browser_without_known_title_uses_browser_name():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.advance(d, 8)
    assert w.events == [("start", "Firefox")]


def test_call_continues_when_title_gone():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.titles["firefox"] = "Meet"
    w.advance(d, 8)
    w.titles["firefox"] = None
    w.advance(d, 600)
    assert w.events == [("start", "Meet")]


def test_disabled_platform_ignored():
    w = World()
    d = w.detector()
    w.disabled = {"discord"}
    w.mic = {DISCORD}
    w.advance(d, 30)
    assert w.events == []


def test_whatsapp_voice_message_ignored_call_detected():
    # WhatsApp usa el micro también al grabar una nota de voz (~20 s en la prueba del dueño):
    # solo cuenta como llamada tras 30 s seguidos de micro.
    w = World()
    d = w.detector()
    w.mic = {WHATSAPP}
    w.advance(d, 24)  # nota de voz de 24 s
    w.mic = set()
    w.advance(d, 20)
    assert w.events == []
    w.mic = {WHATSAPP}  # llamada
    w.advance(d, 28)
    assert w.events == []
    w.advance(d, 4)
    assert w.events == [("start", "WhatsApp")]


def test_second_platform_does_not_switch_active_call():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    w.advance(d, 8)
    w.mic = {TEAMS, CHROME}
    w.advance(d, 30)
    assert w.events == [("start", "Teams")]
    w.mic = {CHROME}  # Teams suelta el micro: termina su llamada
    w.advance(d, 14)
    assert w.events[:2] == [("start", "Teams"), ("stop", "Teams")]


def test_stop_after_grace():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    w.advance(d, 8)
    w.mic = set()
    w.advance(d, 6)
    assert ("stop", "Teams") not in w.events
    w.advance(d, 8)
    assert w.events[-1] == ("stop", "Teams")


def test_brief_mic_release_no_stop():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    w.advance(d, 8)
    w.mic = set()
    w.advance(d, 4)
    w.mic = {TEAMS}
    w.advance(d, 30)
    assert w.events == [("start", "Teams")]


def test_suspended_no_start():
    w = World()
    d = w.detector()
    d.suspend()
    w.mic = {TEAMS}
    w.advance(d, 20)
    assert w.events == []
    d.resume()
    w.advance(d, 8)
    assert w.events == [("start", "Teams")]


def test_manual_stop_dismisses_browser_until_release():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.advance(d, 8)
    d.dismiss()
    w.advance(d, 60)
    assert w.events == [("start", "Firefox")]
    w.mic = set()
    w.advance(d, 14)
    assert w.events == [("start", "Firefox")]  # sin aviso de fin
    w.mic = {FIREFOX}
    w.advance(d, 8)
    assert w.events == [("start", "Firefox"), ("start", "Firefox")]


def test_detected_carries_platform_and_exe():
    got = []
    w = World()
    d = CallDetector(on_start=got.append, on_stop=lambda x: None, probe_mic=lambda: {DISCORD},
                     probe_title=lambda p: None, clock=lambda: w.t)
    w.advance(d, 8)
    assert got[0].platform is platform_for_key(DISCORD) and got[0].exe == "discord.exe"


def test_mic_in_use_parses_registry():
    entries = {
        TEAMS: (134372538113007236, 0),
        "Microsoft.WindowsCamera_8wekyb3d8bbwe": (134372538113007236, 134372538423340618),
        FIREFOX: (134372538113007236, 0),
        "NonPackaged/C:#never#used.exe": (0, 0),
    }
    assert mic_in_use(reader=lambda: entries) == {TEAMS, FIREFOX}


def test_dismissed_browser_does_not_hide_other_calls():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.advance(d, 8)
    d.dismiss()  # la web del navegador no era una llamada: parada a mano
    w.mic = {FIREFOX, TEAMS}  # la pestaña sigue con el micro y entra una llamada de Teams
    w.advance(d, 8)
    assert w.events == [("start", "Firefox"), ("start", "Teams")]


def test_dismiss_in_use_blocks_restart_until_release():
    # Al parar a mano una "Grabar llamada" durante una llamada del navegador, no debe
    # empezar otra grabación de esa misma llamada al reanudarse la detección.
    w = World()
    d = w.detector()
    d.suspend()
    w.mic = {CHROME}
    w.advance(d, 20)
    d.dismiss_in_use()
    d.resume()
    w.advance(d, 30)
    assert w.events == []
    w.mic = set()
    w.advance(d, 14)
    w.mic = {CHROME}
    w.advance(d, 8)
    assert w.events == [("start", "Chrome")]


def test_dismiss_without_active_call_is_harmless():
    w = World()
    d = w.detector()
    d.dismiss()
    w.mic = {TEAMS}
    w.advance(d, 8)
    w.mic = set()
    w.advance(d, 14)
    assert w.events == [("start", "Teams"), ("stop", "Teams")]
