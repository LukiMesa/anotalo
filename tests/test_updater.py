import hashlib
import io

import pytest

from calltx import updater
from calltx.updater import Release, UpdateError, UpdateManager, download, fetch_latest, is_newer, parse_version

URL = "https://github.com/LukiMesa/call-transcriber/releases/download/v0.6.0/Transcriptor-Setup-0.6.0.exe"
PAYLOAD = b"instalador de prueba" * 1000
SHA = hashlib.sha256(PAYLOAD).hexdigest()


def release(**kw):
    base = dict(version="0.6.0", title="Transcriptor 0.6.0", url=URL, name="Transcriptor-Setup-0.6.0.exe",
                size=len(PAYLOAD), sha256=SHA)
    base.update(kw)
    return Release(**base)


class Resp(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()


def test_version_compare():
    assert parse_version("v0.6.0") == (0, 6, 0)
    assert is_newer("0.10.0", "0.9.0")
    assert is_newer("v0.6.1", "0.6.0")
    assert not is_newer("0.6.0", "0.6.0")
    assert not is_newer("0.5.9", "0.6.0")


def test_fetch_latest_picks_setup_asset_and_digest():
    data = {
        "tag_name": "v0.6.0", "name": "Transcriptor 0.6.0",
        "assets": [
            {"name": "notas.txt", "browser_download_url": "https://x/notas.txt", "size": 3},
            {"name": "Transcriptor-Setup-0.6.0.exe", "browser_download_url": URL, "size": 123,
             "digest": "sha256:" + "ab" * 32},
        ],
    }
    r = fetch_latest(fetch_json=lambda url: data)
    assert (r.version, r.url, r.size, r.sha256) == ("0.6.0", URL, 123, "ab" * 32)
    assert fetch_latest(fetch_json=lambda url: {**data, "assets": []}) is None


def test_download_verifies_size_and_hash(tmp_path):
    seen = []
    path = download(release(), tmp_path, seen.append, opener=lambda url: Resp(PAYLOAD))
    assert path.read_bytes() == PAYLOAD and path.name == "Transcriptor-Setup-0.6.0.exe"
    assert seen[-1] == 1.0 and not list(tmp_path.glob("*.part"))


def test_download_rejects_bad_hash_and_deletes(tmp_path):
    with pytest.raises(UpdateError):
        download(release(sha256="00" * 32), tmp_path, lambda p: None, opener=lambda url: Resp(PAYLOAD))
    with pytest.raises(UpdateError):  # descarga cortada
        download(release(), tmp_path, lambda p: None, opener=lambda url: Resp(PAYLOAD[:100]))
    assert list(tmp_path.iterdir()) == []


def test_download_rejects_foreign_url(tmp_path):
    for bad in ("https://evil.example/Transcriptor-Setup-0.6.0.exe",
                "http://github.com/LukiMesa/call-transcriber/releases/download/v0.6.0/Transcriptor-Setup-0.6.0.exe",
                "https://github.com/otro/call-transcriber/releases/download/v0.6.0/Transcriptor-Setup-0.6.0.exe",
                URL.replace("Setup-0.6.0.exe", "Setup-0.6.0.exe.zip")):
        with pytest.raises(UpdateError):
            download(release(url=bad), tmp_path, lambda p: None, opener=lambda url: pytest.fail("no debe descargar"))


class Env:
    def __init__(self, latest="0.6.0", recording=False):
        self.latest = latest
        self.recording = recording
        self.notes, self.installed, self.notified = [], [], []
        self.fail_fetch = False

    def manager(self, notified=""):
        def fetch():
            if self.fail_fetch:
                raise OSError("sin conexión")
            return release(version=self.latest) if self.latest else None

        return UpdateManager(
            "0.5.0", is_recording=lambda: self.recording, notify=lambda *a: self.notes.append(a),
            fetch=fetch, downloader=lambda rel, dest, progress: (progress(1.0), f"setup-{rel.version}.exe")[1],
            installer=self.installed.append, notified_version=notified, on_notified=self.notified.append,
        )


def test_check_background_errors_are_silent_user_errors_shown():
    e = Env()
    e.fail_fetch = True
    m = e.manager()
    m.check(user=False)
    assert m.state == "idle" and m.error is None
    m.check(user=True)
    assert m.state == "error" and "sin conexión" in m.error
    e.fail_fetch = False
    m.check(user=True)
    assert m.state == "available" and m.release.version == "0.6.0"


def test_latest_version_reported_only_when_user_asks():
    e = Env(latest="0.5.0")
    m = e.manager()
    m.check(user=False)
    assert m.state == "idle"
    m.check(user=True)
    assert m.state == "latest"


def test_install_waits_for_recording_end_and_runs_once():
    e = Env(recording=True)
    m = e.manager()
    m.check(user=True)
    m.install()
    m.install()
    assert m.state == "waiting" and e.installed == []
    e.recording = False
    m.on_recording_end()
    m.on_recording_end()
    m.install()
    assert e.installed == ["setup-0.6.0.exe"] and m.state == "installing"


def test_install_without_recording_runs_now():
    e = Env()
    m = e.manager()
    m.check()
    m.install()
    assert e.installed == ["setup-0.6.0.exe"]


def test_notifies_once_per_version():
    e = Env()
    m = e.manager(notified="")
    m.check()
    m.check()
    assert len(e.notes) == 1 and e.notified == ["0.6.0"]
    e2 = Env()
    m2 = e2.manager(notified="0.6.0")  # ya avisado en un arranque anterior
    m2.check()
    assert e2.notes == [] and m2.state == "available"


def test_download_error_is_reported_and_can_retry():
    e = Env()
    m = e.manager()
    calls = []

    def flaky(rel, dest, progress):
        calls.append(1)
        if len(calls) == 1:
            raise UpdateError("descarga dañada")
        return "ok.exe"

    m._download = flaky
    m.check()
    m.install()
    assert m.state == "error" and "dañada" in m.error and e.installed == []
    m.install()
    assert e.installed == ["ok.exe"]


def test_installer_args():
    got = {}
    updater.run_installer(updater.Path("C:/x/setup.exe"), launcher=lambda args, **kw: got.update(args=args, kw=kw))
    assert got["args"][1:] == ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]


def test_first_run_after_update_notifies_once():
    from calltx.config import Settings
    from calltx.updater import announce_if_updated

    s = Settings()
    notes = []
    assert announce_if_updated(s, "0.6.0", lambda *a: notes.append(a)) is True  # primera vez: guarda, sin aviso
    assert notes == [] and s.last_run_version == "0.6.0"
    assert announce_if_updated(s, "0.6.0", lambda *a: notes.append(a)) is False
    assert announce_if_updated(s, "0.6.1", lambda *a: notes.append(a)) is True
    assert notes == [("Transcriptor actualizado", "Ya tienes la versión 0.6.1")]
    assert s.last_run_version == "0.6.1"


def test_recording_started_during_download_waits_and_reuses_file():
    e = Env()
    m = e.manager()
    downloads = []

    def dl(rel, dest, progress):
        downloads.append(1)
        e.recording = True  # empieza otra llamada mientras se descarga
        return "setup-0.6.0.exe"

    m._download = dl
    m.check()
    m.install()
    assert m.state == "waiting" and e.installed == []
    e.recording = False
    m.on_recording_end()
    assert e.installed == ["setup-0.6.0.exe"] and downloads == [1]  # no vuelve a descargar


def test_installing_that_never_closes_the_app_becomes_error():
    import time

    e = Env()
    m = e.manager()
    m._installing_timeout_s = 0.05
    m.check()
    m.install()
    assert m.state == "installing"
    time.sleep(0.3)
    assert m.state == "error" and m.error
    m.install()  # se puede reintentar
    assert e.installed == ["setup-0.6.0.exe", "setup-0.6.0.exe"]


NEW_URL = "https://github.com/LukiMesa/anotalo/releases/download/v1.0.0/Anotalo-Setup-1.0.0.exe"


def test_fetch_latest_prefers_anotalo_asset():
    data = {"tag_name": "v1.0.0", "name": "Anotalo 1.0.0", "assets": [
        {"name": "Transcriptor-Setup-1.0.0.exe", "size": 1,
         "browser_download_url": "https://github.com/LukiMesa/anotalo/releases/download/v1.0.0/Transcriptor-Setup-1.0.0.exe"},
        {"name": "Anotalo-Setup-1.0.0.exe", "browser_download_url": NEW_URL, "size": 2, "digest": "sha256:" + "cd" * 32},
    ]}
    r = fetch_latest(fetch_json=lambda url: data)
    assert (r.version, r.name, r.url, r.size, r.sha256) == ("1.0.0", "Anotalo-Setup-1.0.0.exe", NEW_URL, 2, "cd" * 32)


def test_download_accepts_renamed_repo_and_asset(tmp_path):
    rel = release(version="1.0.0", url=NEW_URL, name="Anotalo-Setup-1.0.0.exe")
    assert download(rel, tmp_path, lambda p: None, opener=lambda url: Resp(PAYLOAD)).name == "Anotalo-Setup-1.0.0.exe"


def test_download_still_rejects_lookalikes(tmp_path):
    for url, name in (
        (NEW_URL, "Transcriptor-Setup-1.0.0.exe"),  # el nombre no coincide con la URL
        (NEW_URL.replace("LukiMesa/anotalo", "otro/anotalo"), "Anotalo-Setup-1.0.0.exe"),
        (NEW_URL.replace("/anotalo/", "/anotalo-fake/"), "Anotalo-Setup-1.0.0.exe"),
        (NEW_URL.replace("Anotalo-Setup", "Anotalo-Setup-x"), "Anotalo-Setup-x-1.0.0.exe"),
    ):
        with pytest.raises(UpdateError):
            download(release(version="1.0.0", url=url, name=name), tmp_path, lambda p: None,
                     opener=lambda url: pytest.fail("no debe descargar"))
