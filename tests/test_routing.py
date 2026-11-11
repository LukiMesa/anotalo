from calltx.audio.routing import SessionInfo, pick_capture, pick_render, process_key

TEAMS = "MSTeams_8wekyb3d8bbwe"
CHROME = "NonPackaged/#chrome.exe"
SPOTIFY = "NonPackaged/#spotify.exe"


def s(endpoint, key, active=True):
    return SessionInfo(endpoint_id=endpoint, endpoint_name=endpoint.upper(), pid=1, active=active, key=key)


def test_process_key_matches_registry_style():
    assert process_key("ms-teams.exe", "MSTeams_8wekyb3d8bbwe") == TEAMS
    assert process_key("Chrome.exe", None) == CHROME
    assert process_key(None, None) is None


def test_render_follows_the_call_app_to_its_device():
    # Bluetooth en manos libres: Teams suena ahí aunque el predeterminado sean los altavoces
    sessions = [s("spk", SPOTIFY), s("bt-hf", TEAMS), s("spk", TEAMS, active=False)]
    ep, why = pick_render(sessions, "teams", comm="spk", console="spk")
    assert ep == "bt-hf" and "Teams" in why


def test_render_prefers_default_devices_when_the_app_plays_on_several():
    sessions = [s("a", TEAMS), s("b", TEAMS)]
    assert pick_render(sessions, "teams", comm="b", console="a")[0] == "b"
    assert pick_render(sessions, "teams", comm=None, console="a")[0] == "a"


def test_manual_call_picks_any_call_app():
    sessions = [s("spk", SPOTIFY), s("bt", CHROME)]
    ep, why = pick_render(sessions, None, comm="spk", console="spk")
    assert ep == "bt" and "Chrome" in why


def test_render_falls_back_to_devices_with_sound_then_defaults():
    # sin app de llamadas sonando: el de comunicaciones si suena, luego el predeterminado, luego cualquiera
    assert pick_render([s("comm", SPOTIFY), s("con", SPOTIFY)], "teams", comm="comm", console="con")[0] == "comm"
    assert pick_render([s("con", SPOTIFY)], "teams", comm="comm", console="con")[0] == "con"
    assert pick_render([s("bt", SPOTIFY)], None, comm="comm", console="con")[0] == "bt"
    assert pick_render([], None, comm="comm", console="con")[0] == "con"
    assert pick_render([s("x", SPOTIFY, active=False)], None, comm="comm", console=None)[0] == "comm"
    assert pick_render([], None, comm=None, console=None)[0] is None


def test_capture_uses_the_mic_of_the_call_app():
    sessions = [s("laptop-mic", "NonPackaged/#python.exe"), s("bt-mic", TEAMS)]
    assert pick_capture(sessions, "teams", console="laptop-mic")[0] == "bt-mic"
    assert pick_capture(sessions, None, console="laptop-mic")[0] == "bt-mic"
    assert pick_capture(sessions, "zoom", console="laptop-mic")[0] == "laptop-mic"
    assert pick_capture([], "teams", console=None)[0] is None
