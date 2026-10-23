from calltx.detector import CallApp, CallDetector, mic_in_use

TEAMS = "MSTeams_8wekyb3d8bbwe"
FIREFOX = "NonPackaged/C:#Program Files#Mozilla Firefox#firefox.exe"


class World:
    def __init__(self):
        self.t = 0.0
        self.mic: set[str] = set()
        self.meet = False
        self.events: list[tuple[str, CallApp]] = []

    def detector(self):
        return CallDetector(
            on_start=lambda a: self.events.append(("start", a)),
            on_stop=lambda a: self.events.append(("stop", a)),
            probe_mic=lambda: set(self.mic),
            probe_meet=lambda: self.meet,
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
    assert w.events == [("start", CallApp.TEAMS)]


def test_teams_running_without_mic_no_start():
    w = World()
    d = w.detector()
    w.advance(d, 60)
    assert w.events == []


def test_firefox_mic_without_meet_no_start():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.meet = False
    w.advance(d, 60)
    assert w.events == []


def test_meet_starts():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.meet = True
    w.advance(d, 8)
    assert w.events == [("start", CallApp.MEET)]


def test_meet_continues_when_title_gone():
    w = World()
    d = w.detector()
    w.mic = {FIREFOX}
    w.meet = True
    w.advance(d, 8)
    w.meet = False
    w.advance(d, 600)
    assert w.events == [("start", CallApp.MEET)]


def test_stop_after_grace():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    w.advance(d, 8)
    w.mic = set()
    w.advance(d, 6)
    assert ("stop", CallApp.TEAMS) not in w.events
    w.advance(d, 8)
    assert w.events[-1] == ("stop", CallApp.TEAMS)


def test_brief_mic_release_no_stop():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    w.advance(d, 8)
    w.mic = set()
    w.advance(d, 4)
    w.mic = {TEAMS}
    w.advance(d, 30)
    assert w.events == [("start", CallApp.TEAMS)]


def test_suspended_no_start():
    w = World()
    d = w.detector()
    d.suspend()
    w.mic = {TEAMS}
    w.advance(d, 20)
    assert w.events == []
    d.resume()
    w.advance(d, 8)
    assert w.events == [("start", CallApp.TEAMS)]


def test_mic_in_use_parses_registry():
    entries = {
        TEAMS: (134372538113007236, 0),
        "Microsoft.WindowsCamera_8wekyb3d8bbwe": (134372538113007236, 134372538423340618),
        FIREFOX: (134372538113007236, 0),
        "NonPackaged/C:#never#used.exe": (0, 0),
    }
    assert mic_in_use(reader=lambda: entries) == {TEAMS, FIREFOX}


def test_dismissed_call_not_restarted_and_no_stop_event():
    w = World()
    d = w.detector()
    w.mic = {TEAMS}
    w.advance(d, 8)
    d.dismiss()
    w.advance(d, 60)
    assert w.events == [("start", CallApp.TEAMS)]
    w.mic = set()
    w.advance(d, 14)
    assert w.events == [("start", CallApp.TEAMS)]
    w.mic = {TEAMS}
    w.advance(d, 8)
    assert w.events == [("start", CallApp.TEAMS), ("start", CallApp.TEAMS)]


def test_meet_title_matching():
    from calltx.detector import is_meet_title

    assert is_meet_title("Meet - abc-defg-hij — Mozilla Firefox")
    assert is_meet_title("Meet – Reunión semanal — Mozilla Firefox")
    assert is_meet_title("Google Meet — Mozilla Firefox")
    assert not is_meet_title("Meeting notes – Google Docs — Mozilla Firefox")
    assert not is_meet_title("Meetup: Python Madrid — Mozilla Firefox")


def test_detector_disabled_app():
    w = World()
    d = CallDetector(
        on_start=lambda a: w.events.append(("start", a)),
        on_stop=lambda a: w.events.append(("stop", a)),
        probe_mic=lambda: set(w.mic),
        probe_meet=lambda: w.meet,
        clock=lambda: w.t,
        enabled=lambda app: app is not CallApp.TEAMS,
    )
    w.mic = {TEAMS}
    w.advance(d, 20)
    assert w.events == []
    w.mic = {FIREFOX}
    w.meet = True
    w.advance(d, 8)
    assert w.events == [("start", CallApp.MEET)]
