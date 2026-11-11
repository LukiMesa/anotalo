"""Comprobaciones del script del instalador (Inno Setup) que no se pueden probar sin instalar."""
from pathlib import Path

ISS = (Path(__file__).parent.parent / "packaging" / "installer.iss").read_text(encoding="utf-8")


def test_taskkill_does_not_kill_its_own_installer():
    # La app lanza el instalador como proceso hijo: "taskkill /T" mataba también al instalador.
    line = next(l for l in ISS.splitlines() if "taskkill.exe" in l)
    assert "/IM Transcriptor.exe" in line and "/T " not in line and "/T'" not in line


def test_failed_silent_update_relaunches_app():
    assert "procedure DeinitializeSetup" in ISS
    body = ISS.split("procedure DeinitializeSetup", 1)[1]
    assert "--minimized" in body and "WizardSilent" in body
