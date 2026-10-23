# Publica el instalador como Release en GitHub (repositorio privado).
# Uso: .\scripts\release.ps1   (antes: .\scripts\build.ps1)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$version = & (Join-Path $root ".venv\Scripts\python.exe") -c "import calltx; print(calltx.__version__)"
$setup = "dist\Transcriptor-Setup-$version.exe"
if (-not (Test-Path $setup)) { throw "No existe $setup. Ejecuta antes scripts\build.ps1." }
if (git status --porcelain) { throw "Hay cambios sin guardar en git." }

$tag = "v$version"
if (-not (git tag --list $tag)) { git tag $tag; git push origin $tag }
$notes = @"
Instalador de Transcriptor $version para Windows 10/11 (64 bits).

1. Descarga ``Transcriptor-Setup-$version.exe`` e instálalo (no necesita permisos de administrador).
2. Si Windows muestra "Windows protegió tu PC": *Más información* → *Ejecutar de todas formas* (el instalador no está firmado).
3. La primera vez descarga el modelo (y en PCs con NVIDIA, los componentes de GPU). La barra superior muestra el progreso.
"@
# Las notas van en un archivo: PowerShell 5.1 rompe los argumentos con comillas y acentos
$notesFile = Join-Path $env:TEMP "transcriptor-release-notes.md"
[IO.File]::WriteAllText($notesFile, $notes, (New-Object Text.UTF8Encoding $false))
gh release create $tag $setup --title "Transcriptor $version" --notes-file $notesFile
