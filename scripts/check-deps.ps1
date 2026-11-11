# Comprueba de qué DLL depende el exe (spec §5.1): CRT estático (sin MSVCP/VCRUNTIME/VCOMP/UCRT),
# vulkan-1.dll solo de carga diferida y, por lo demás, únicamente DLL de Windows.
# Uso: .\scripts\check-deps.ps1 -Exe target\release\anotalo.exe
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = "Stop"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$dumpbin = Get-ChildItem (Join-Path $vs "VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe") | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $dumpbin) { throw "No se encontró dumpbin.exe (Build Tools de Visual Studio con C++)" }

$imports = @(); $delay = @(); $section = $null
foreach ($line in (& $dumpbin.FullName /nologo /dependents $Exe)) {
    $t = $line.Trim()
    if ($t -like "Image has the following dependencies*") { $section = "imp"; continue }
    if ($t -like "Image has the following delay load dependencies*") { $section = "delay"; continue }
    if ($t -like "Summary*") { $section = $null; continue }
    if ($section -and $t -match '^\S+\.dll$') {
        if ($section -eq "imp") { $imports += $t } else { $delay += $t }
    }
}

# DLL de Windows permitidas (todas vienen con el sistema)
$allowed = @(
    "KERNEL32.dll", "USER32.dll", "GDI32.dll", "ADVAPI32.dll", "SHELL32.dll", "ole32.dll", "OLEAUT32.dll",
    "WS2_32.dll", "bcrypt.dll", "bcryptprimitives.dll", "ntdll.dll", "CRYPT32.dll", "Secur32.dll", "dbghelp.dll"
)
$crt = '^(MSVCP|VCRUNTIME|VCOMP|api-ms-win-crt-)'

"Imports:"; $imports | ForEach-Object { "  $_" }
"Delay-load:"; $delay | ForEach-Object { "  $_" }

$problems = @()
foreach ($dll in $imports) {
    if ($dll -match $crt) { $problems += "$dll es del runtime de C/C++: falta CRT estático" }
    elseif ($dll -ieq "vulkan-1.dll") { $problems += "vulkan-1.dll debe ser de carga diferida, no una importación normal" }
    elseif ($dll -notlike "api-ms-win-core-*" -and -not ($allowed | Where-Object { $_ -ieq $dll })) { $problems += "$dll no está en la lista de DLL de Windows permitidas" }
}
foreach ($dll in $delay) {
    if ($dll -ine "vulkan-1.dll") { $problems += "$dll no debería ser de carga diferida" }
}
if (-not ($delay | Where-Object { $_ -ieq "vulkan-1.dll" })) { $problems += "falta vulkan-1.dll entre las de carga diferida" }

if ($problems) { $problems | ForEach-Object { "FALLO: $_" }; exit 1 }
"OK"
