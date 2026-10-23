# Compila Transcriptor.exe (PyInstaller) y el instalador (Inno Setup).
# Uso: .\scripts\build.ps1            → dist\Transcriptor-Setup-<versión>.exe
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$py = Join-Path $root ".venv\Scripts\python.exe"

$version = & $py -c "import calltx; print(calltx.__version__)"
Write-Host "Transcriptor $version"

& $py -m pip install -q "pyinstaller>=6.10"
& $py -m pytest -q
if ($LASTEXITCODE -ne 0) { throw "Los tests fallan: no se compila." }

& $py -m PyInstaller packaging\transcriptor.spec --noconfirm --clean --distpath dist --workpath build
if ($LASTEXITCODE -ne 0) { throw "PyInstaller falló." }

# Autodiagnóstico del ejecutable (sin ventana; escribe el resultado en un archivo)
$log = Join-Path $env:TEMP "transcriptor-selftest.txt"
Remove-Item $log -ErrorAction SilentlyContinue
$p = Start-Process -FilePath "dist\Transcriptor\Transcriptor.exe" -ArgumentList "--selftest" -Wait -PassThru
Get-Content $log -ErrorAction SilentlyContinue
if ($p.ExitCode -ne 0) { throw "El ejecutable no pasó el autodiagnóstico." }

$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Falta Inno Setup 6 (winget install JRSoftware.InnoSetup)." }
& $iscc "/DAppVersion=$version" "packaging\installer.iss"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup falló." }
Write-Host "Listo: dist\Transcriptor-Setup-$version.exe"
