# Comprueba de qué DLL depende cada exe (spec §5.1): CRT estático (sin MSVCP/VCRUNTIME/VCOMP/UCRT),
# vulkan-1.dll y vcomp140.dll solo de carga diferida y, por lo demás, únicamente DLL de Windows, cargadas solo de
# System32 (/DEPENDENTLOADFLAG:0x800: una DLL con el mismo nombre junto al exe no se carga).
# Uso: .\scripts\check-deps.ps1 -Exe target\release\anotalo.exe[,target\release\examples\bench.exe]
param([Parameter(Mandatory = $true)][string[]]$Exe)
$ErrorActionPreference = "Stop"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$dumpbin = Get-ChildItem (Join-Path $vs "VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe") | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $dumpbin) { throw "No se encontró dumpbin.exe (Build Tools de Visual Studio con C++)" }

# DLL de Windows permitidas (todas vienen con el sistema)
$allowed = @(
    "KERNEL32.dll", "USER32.dll", "GDI32.dll", "ADVAPI32.dll", "SHELL32.dll", "ole32.dll", "OLEAUT32.dll",
    "WS2_32.dll", "bcrypt.dll", "bcryptprimitives.dll", "ntdll.dll", "CRYPT32.dll", "Secur32.dll", "dbghelp.dll",
    "combase.dll", "mmdevapi.dll", "propsys.dll", "psapi.dll",
    # ventana principal (M3): dwmapi (esquinas, modo oscuro y sombra), y para WebView2 vía wry,
    # shlwapi (SHCreateMemStream) y comctl32 (SetWindowSubclass; la 5.82 de System32 la tiene)
    "dwmapi.dll", "shlwapi.dll", "comctl32.dll"
)
$crt = '^(MSVCP|VCRUNTIME|VCOMP|api-ms-win-crt-)'

$problems = @()
foreach ($file in $Exe) {
    $imports = @(); $delay = @(); $section = $null
    foreach ($line in (& $dumpbin.FullName /nologo /dependents $file)) {
        $t = $line.Trim()
        if ($t -like "Image has the following dependencies*") { $section = "imp"; continue }
        if ($t -like "Image has the following delay load dependencies*") { $section = "delay"; continue }
        if ($t -like "Summary*") { $section = $null; continue }
        if ($section -and $t -match '^\S+\.dll$') {
            if ($section -eq "imp") { $imports += $t } else { $delay += $t }
        }
    }

    "== $file"
    "Imports:"; $imports | ForEach-Object { "  $_" }
    "Delay-load:"; $delay | ForEach-Object { "  $_" }

    foreach ($dll in $imports) {
        if ($dll -match $crt) { $problems += "${file}: $dll es del runtime de C/C++: falta CRT estático" }
        elseif ($dll -ieq "vulkan-1.dll") { $problems += "${file}: vulkan-1.dll debe ser de carga diferida, no una importación normal" }
        elseif ($dll -notlike "api-ms-win-core-*" -and -not ($allowed | Where-Object { $_ -ieq $dll })) { $problems += "${file}: $dll no está en la lista de DLL de Windows permitidas" }
    }
    # vcomp140.dll (OpenMP de Microsoft, firmada por Microsoft) va junto al exe: solo de carga
    # diferida, y anotalo-whisper la carga de la carpeta del exe (como importación normal, Windows
    # la buscaría en System32 por /DEPENDENTLOADFLAG y no la encontraría en un PC sin el VC++ redist).
    foreach ($dll in $delay) {
        if ($dll -ine "vulkan-1.dll" -and $dll -ine "vcomp140.dll") { $problems += "${file}: $dll no debería ser de carga diferida" }
    }
    if (-not ($delay | Where-Object { $_ -ieq "vulkan-1.dll" })) { $problems += "${file}: falta vulkan-1.dll entre las de carga diferida" }
    $flag = (& $dumpbin.FullName /nologo /loadconfig $file) | Where-Object { $_ -match '^\s*([0-9A-Fa-f]+) Dependent Load Flag' } | ForEach-Object { [Convert]::ToInt32($Matches[1], 16) }
    "Dependent Load Flag: $flag"
    if (-not ($flag -band 0x800)) { $problems += "${file}: falta /DEPENDENTLOADFLAG:0x800 (las DLL importadas deben cargarse solo de System32)" }
}

if ($problems) { $problems | ForEach-Object { "FALLO: $_" }; exit 1 }
"OK"
