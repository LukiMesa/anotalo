# PyInstaller: carpeta dist/Transcriptor con Transcriptor.exe (sin consola).
# Uso: pyinstaller packaging/transcriptor.spec --noconfirm   (lo llama scripts/build.ps1)
from pathlib import Path

from PyInstaller.utils.hooks import collect_data_files, collect_dynamic_libs, collect_submodules

ROOT = Path(SPECPATH).parent

datas = [
    (str(ROOT / "calltx" / "ui" / "web"), "calltx/ui/web"),
    (str(ROOT / "calltx" / "ui" / "assets"), "calltx/ui/assets"),
]
datas += collect_data_files("faster_whisper")  # incluye el modelo Silero (VAD)
datas += collect_data_files("webview")
datas += collect_data_files("onnxruntime")

binaries = []
for pkg in ("ctranslate2", "onnxruntime", "pyaudiowpatch", "soxr"):
    binaries += collect_dynamic_libs(pkg)

hiddenimports = (
    collect_submodules("calltx")
    + collect_submodules("webview.platforms")
    + ["pystray._win32", "comtypes.stream", "clr", "winotify"]
)

a = Analysis(
    [str(ROOT / "packaging" / "entry.py")],
    pathex=[str(ROOT)],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    excludes=["nvidia", "torch", "tkinter", "matplotlib", "pytest", "IPython", "pandas"],
    noarchive=False,
)
pyz = PYZ(a.pure)
exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="Transcriptor",
    console=False,
    icon=str(ROOT / "calltx" / "ui" / "assets" / "icon.ico"),
    version=None,
)
coll = COLLECT(exe, a.binaries, a.datas, name="Transcriptor", upx=False)
