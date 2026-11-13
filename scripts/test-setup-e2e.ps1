# Prueba de punta a punta del instalador (M4, job setup-e2e del workflow Anotalo):
#   1-4. Transcriptor 0.6.4 instalado y abierto se actualiza a Anotalo con el propio código de
#        actualización de 0.6.4 (scripts/fixtures/update_chain.py): Transcriptor desaparece, Anotalo
#        queda abierto con sus ajustes, y los de Transcriptor y las grabaciones no cambian.
#   5.   Actualización con la app y un `Anotalo.exe mcp` abiertos.
#   6.   Desinstalación silenciosa: las grabaciones y los ajustes se quedan.
#
# INSTALA Y DESINSTALA DE VERDAD en el usuario que la ejecuta, y cierra Transcriptor y Anotalo. Solo
# corre en GitHub Actions (GITHUB_ACTIONS=true) o con -ThisPcIsDisposable, en un PC desechable
# recién preparado.
#
# Uso: .\scripts\test-setup-e2e.ps1 -Setup dist\anotalo.exe
#   Necesita gh (con GH_TOKEN) para bajar Transcriptor 0.6.4 y python 3.12 en el PATH.
param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [string]$Python = "python",
    [switch]$ThisPcIsDisposable
)
$ErrorActionPreference = "Stop"

# Lo primero, antes de tocar nada.
if ($env:GITHUB_ACTIONS -ne "true" -and -not $ThisPcIsDisposable) {
    throw "Esta prueba instala y desinstala Transcriptor y Anotalo de verdad. Solo en CI (GITHUB_ACTIONS=true) o con -ThisPcIsDisposable en un PC desechable."
}

$Setup = (Resolve-Path -LiteralPath $Setup).Path
$repo = Split-Path $PSScriptRoot -Parent

# --- dónde va cada cosa -----------------------------------------------------------------------
$work = Join-Path ([IO.Path]::GetTempPath()) "anotalo-e2e"
$local = $env:LOCALAPPDATA
$legacyData = Join-Path $env:APPDATA "CallTranscriber"
$legacyCfg = Join-Path $legacyData "config.json"
$legacyLog = Join-Path $legacyData "app.log"
$legacyLocal = Join-Path $local "Transcriptor"
$innoDir = Join-Path $local "Programs\Transcriptor"
$uninstallRoot = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall"
$innoKey = "$uninstallRoot\{6B7C2D51-3A8E-4F5B-9C1D-2E4F6A8B0C13}_is1"
$anotaloKey = "$uninstallRoot\Anotalo"
$installDir = Join-Path $local "Programs\Anotalo"
$exe = Join-Path $installDir "Anotalo.exe"
$data = Join-Path $env:APPDATA "Anotalo"
$cfg = Join-Path $data "config.json"
$appLog = Join-Path $data "app.log"
$setupLog = Join-Path $data "setup.log"
$programs = [Environment]::GetFolderPath("Programs")
$startup = [Environment]::GetFolderPath("Startup")
$desktop = [Environment]::GetFolderPath("DesktopDirectory")
$recDir = "C:\e2e\Llamadas"
$rec = Join-Path $recDir "2026-11-10_0900_Teams.md"
$hotkey = "<ctrl>+<alt>+t"
# Transcriptor-Setup-0.6.4.exe tal como se publicó (la Release v0.6.4 no cambia)
$innoSha = "5EB54DC7FFAAFE3E6AE31F9D75FD1BEC6913E8E56113CF5ECFAE7F5AA4797C3D"

# --- ayudas -----------------------------------------------------------------------------------
function Step([int]$n, [string]$text) { Write-Host ""; Write-Host "PASO ${n}: $text" }
function Fail([string]$msg) { throw "FALLO: $msg" }
function Check([bool]$ok, [string]$msg) { if (-not $ok) { Fail $msg }; Write-Host "  ok: $msg" }
function Sha([string]$path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash }

# Lee un archivo que otro proceso puede tener abierto (app.log, config.json).
function Read-Shared([string]$path) {
    $fs = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]"ReadWrite, Delete")
    try { (New-Object IO.StreamReader($fs, [Text.Encoding]::UTF8)).ReadToEnd() } finally { $fs.Dispose() }
}

function Get-Procs([string]$name) {
    @(Get-CimInstance -ClassName Win32_Process -Filter "Name = '$name'")
}

# La app instalada (no los `Anotalo.exe mcp`).
function Get-AppProcs {
    @(Get-Procs "Anotalo.exe" | Where-Object { $_.ExecutablePath -ieq $exe -and $_.CommandLine -notmatch '\smcp\s*$' })
}

# Espera a que se cumplan todas las comprobaciones; si no, falla con las que no se cumplen.
function Wait-All([int]$seconds, $checks) {
    $deadline = [DateTime]::UtcNow.AddSeconds($seconds)
    while ($true) {
        $failing = @(foreach ($name in $checks.Keys) {
                $ok = $false
                try { $ok = [bool](& $checks[$name]) } catch { $ok = $false }
                if (-not $ok) { $name }
            })
        if ($failing.Count -eq 0) { break }
        if ([DateTime]::UtcNow -gt $deadline) { Fail ("tras $seconds s: " + ($failing -join "; ")) }
        Start-Sleep -Milliseconds 500
    }
    foreach ($name in $checks.Keys) { Write-Host "  ok: $name" }
}

# Lanza un exe con CreateProcess (sin ShellExecute: ni SmartScreen ni el aviso de la marca de la web).
function Start-Exe([string]$file, [string]$arguments, [switch]$Redirect) {
    $psi = New-Object Diagnostics.ProcessStartInfo $file
    $psi.Arguments = $arguments
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $work
    if ($Redirect) {
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
    }
    [Diagnostics.Process]::Start($psi)
}

# Un programa de consola (o uno que registra por stderr): vale su código de salida, no su stderr.
function Invoke-Native([scriptblock]$block) {
    $ErrorActionPreference = "Continue"
    & $block
    $LASTEXITCODE
}

function Show-Diagnostics {
    Write-Host ""
    Write-Host "--- Diagnóstico"
    foreach ($f in $setupLog, $appLog, $legacyLog) {
        if (Test-Path -LiteralPath $f) {
            Write-Host "--- $f (final)"
            try { (Read-Shared $f) -split "`r?`n" | Select-Object -Last 60 | ForEach-Object { Write-Host $_ } } catch { Write-Host "(no se pudo leer: $_)" }
        }
    }
    Write-Host "--- Procesos"
    foreach ($p in @(Get-Procs "Anotalo.exe") + @(Get-Procs "Transcriptor.exe")) { Write-Host "  $($p.ProcessId) $($p.CommandLine)" }
    foreach ($d in $installDir, $innoDir) {
        if (Test-Path -LiteralPath $d) { Write-Host "--- $d"; Get-ChildItem -LiteralPath $d -Force | ForEach-Object { Write-Host "  $($_.Name)" } }
    }
}

$mcp = $null
$failed = $true
try {
    # --- antes: un usuario sin Transcriptor ni Anotalo ------------------------------------------
    $already = @($legacyData, $legacyLocal, $innoDir, $data, (Join-Path $local "Anotalo"), $installDir, $innoKey, $anotaloKey |
            Where-Object { Test-Path -LiteralPath $_ })
    if ($already) { Fail "este usuario ya tiene Transcriptor o Anotalo ($($already -join ', ')): la prueba necesita uno sin ellos" }
    $running = @(Get-Procs "Anotalo.exe") + @(Get-Procs "Transcriptor.exe")
    if ($running) { Fail "ya hay un Anotalo.exe o un Transcriptor.exe abierto" }
    New-Item -ItemType Directory -Force $work | Out-Null

    Step 1 "ajustes de Transcriptor, una grabación y la caché de CUDA"
    $utf8 = New-Object Text.UTF8Encoding $false
    New-Item -ItemType Directory -Force $legacyData, $recDir, (Join-Path $legacyLocal "cuda") | Out-Null
    [IO.File]::WriteAllText($legacyCfg, '{"hotkey": "<ctrl>+<alt>+t", "output_dir": "C:\\e2e\\Llamadas", "label_me": "Prueba", "autostart": true}', $utf8)
    $call = @(
        "# Llamada — Teams — 2026-11-10 09:00", "",
        "- Inicio: 09:00:04", "- Fin: 09:12:40", "- Duración: 00:12:36", "",
        "---", "",
        "**[00:00:02] Yo:** Buenos días, repasamos el plan E2E-7Q.", "",
        "**[00:00:06] Otros:** Perfecto, empezamos por el presupuesto.", ""
    ) -join "`n"
    [IO.File]::WriteAllText($rec, $call, $utf8)
    [IO.File]::WriteAllBytes((Join-Path $legacyLocal "cuda\x.dll"), [byte[]](1..64))
    Check (Test-Path -LiteralPath $rec) "grabación en $rec"

    Step 2 "instalar Transcriptor 0.6.4 (Inno, en silencio)"
    # Su instalador muestra un cuadro (no suprimible) si falta WebView2: aquí se quedaría esperando.
    $wv2 = @("HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        "HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}") |
        ForEach-Object { (Get-ItemProperty -LiteralPath $_ -Name pv -ErrorAction SilentlyContinue).pv } |
        Where-Object { $_ -and $_ -ne "0.0.0.0" }
    Check ([bool]$wv2) "WebView2 instalado ($($wv2 | Select-Object -First 1))"
    $code = Invoke-Native { gh release download v0.6.4 -R LukiMesa/anotalo -p Transcriptor-Setup-0.6.4.exe -D $work --clobber | Out-Host }
    if ($code -ne 0) { Fail "gh release download terminó con $code" }
    $inno = Join-Path $work "Transcriptor-Setup-0.6.4.exe"
    Check ((Sha $inno) -eq $innoSha) "Transcriptor-Setup-0.6.4.exe es el publicado"
    # con el acceso del escritorio también: así se comprueba que Anotalo quita los tres
    $p = Start-Exe $inno "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /MERGETASKS=desktopicon"
    if (-not $p.WaitForExit(300000)) { $p.Kill(); Fail "el instalador de Transcriptor no terminó en 5 min" }
    Check ($p.ExitCode -eq 0) "el instalador de Transcriptor terminó con 0 (fue $($p.ExitCode))"
    Wait-All 30 ([ordered]@{
            "Programs\Transcriptor\Transcriptor.exe"        = { Test-Path -LiteralPath (Join-Path $innoDir "Transcriptor.exe") }
            "la clave de desinstalación de Inno"            = { Test-Path -LiteralPath $innoKey }
            "Transcriptor de llamadas.lnk en Inicio"        = { Test-Path -LiteralPath (Join-Path $startup "Transcriptor de llamadas.lnk") }
            "Transcriptor.lnk en el menú Inicio"            = { Test-Path -LiteralPath (Join-Path $programs "Transcriptor.lnk") }
            "Transcriptor.lnk en el escritorio"             = { Test-Path -LiteralPath (Join-Path $desktop "Transcriptor.lnk") }
        })
    # su [Run] lo abre (minimizado)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Get-Procs "Transcriptor.exe") -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 500 }
    if (Get-Procs "Transcriptor.exe") {
        Write-Host "  ok: Transcriptor abierto"
        # al arrancar reescribe config.json: se espera a que termine de arrancar
        $deadline = [DateTime]::UtcNow.AddSeconds(60)
        $started = $false
        while (-not $started -and [DateTime]::UtcNow -lt $deadline) {
            Start-Sleep -Seconds 1
            $started = (Test-Path -LiteralPath $legacyLog) -and (Read-Shared $legacyLog).Contains("Transcriptor iniciado")
        }
        if ($started) { Write-Host "  ok: Transcriptor iniciado (app.log)" } else { Write-Warning "Transcriptor no escribió 'Transcriptor iniciado' en 60 s; sigo" }
    }
    else {
        Write-Warning "Transcriptor no se abrió en 30 s; sigo sin él"
    }
    Start-Sleep -Seconds 10
    $legacyCfgSha = Sha $legacyCfg
    $recSha = Sha $rec
    Write-Host "  SHA-256 de los ajustes de Transcriptor: $legacyCfgSha"
    Write-Host "  SHA-256 de la grabación: $recSha"

    Step 3 "el botón Actualizar de 0.6.4 (su updater.py: comprobar, descargar y lanzar el instalador)"
    $code = Invoke-Native { & $Python -I -X utf8 (Join-Path $repo "scripts\fixtures\update_chain.py") --setup $Setup | Out-Host }
    if ($code -ne 0) { Fail "update_chain.py terminó con $code" }

    Step 4 "Anotalo sustituye a Transcriptor"
    Wait-All 120 ([ordered]@{
            "ningún Transcriptor.exe abierto"               = { -not (Get-Procs "Transcriptor.exe") }
            "Anotalo abierto desde $exe"                    = { @(Get-AppProcs).Count -eq 1 }
            "sin la clave de Inno"                          = { -not (Test-Path -LiteralPath $innoKey) }
            "sin Programs\Transcriptor"                     = { -not (Test-Path -LiteralPath $innoDir) }
            "sin Transcriptor.lnk en el menú Inicio"        = { -not (Test-Path -LiteralPath (Join-Path $programs "Transcriptor.lnk")) }
            "sin Transcriptor.lnk en el escritorio"         = { -not (Test-Path -LiteralPath (Join-Path $desktop "Transcriptor.lnk")) }
            "sin Transcriptor de llamadas.lnk en Inicio"    = { -not (Test-Path -LiteralPath (Join-Path $startup "Transcriptor de llamadas.lnk")) }
            "sin %LOCALAPPDATA%\Transcriptor"               = { -not (Test-Path -LiteralPath $legacyLocal) }
            "Anotalo.lnk en el menú Inicio"                 = { Test-Path -LiteralPath (Join-Path $programs "Anotalo.lnk") }
            "Anotalo.lnk en Inicio (Transcriptor arrancaba con Windows)" = { Test-Path -LiteralPath (Join-Path $startup "Anotalo.lnk") }
            "Anotalo.lnk en el escritorio (Transcriptor tenía uno)" = { Test-Path -LiteralPath (Join-Path $desktop "Anotalo.lnk") }
            "la clave de desinstalación de Anotalo"         = { Test-Path -LiteralPath $anotaloKey }
        })
    $appPid = @(Get-AppProcs)[0].ProcessId
    $sig = Get-AuthenticodeSignature -LiteralPath (Join-Path $installDir "vcomp140.dll")
    Check ($sig.Status -eq "Valid" -and $sig.SignerCertificate.Subject -match 'O=Microsoft Corporation') "vcomp140.dll junto al exe, firmada por Microsoft ($($sig.Status))"
    Check ((Sha $legacyCfg) -eq $legacyCfgSha) "los ajustes de Transcriptor no cambiaron"
    Check ((Sha $rec) -eq $recSha) "la grabación no cambió"
    Wait-All 60 ([ordered]@{
            "los ajustes de Anotalo tienen el atajo y la carpeta de Transcriptor" = {
                $c = (Read-Shared $cfg) | ConvertFrom-Json
                $c.hotkey -eq $hotkey -and $c.output_dir -eq $recDir
            }
        })

    Step 5 "actualizar con la app y un 'Anotalo.exe mcp' abiertos"
    $psi = New-Object Diagnostics.ProcessStartInfo $exe
    $psi.Arguments = "mcp"
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $work
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = $utf8
    if ($psi.PSObject.Properties["StandardInputEncoding"]) { $psi.StandardInputEncoding = $utf8 }
    $mcp = [Diagnostics.Process]::Start($psi)
    $mcpErr = $mcp.StandardError.ReadToEndAsync()
    $mcp.StandardInput.AutoFlush = $true
    function Ask-Mcp([string]$json) {
        $mcp.StandardInput.WriteLine($json)
        $line = $mcp.StandardOutput.ReadLineAsync()
        if (-not $line.Wait(15000)) { Fail "el servidor MCP no contestó en 15 s a $json" }
        $line.Result
    }
    $init = Ask-Mcp '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"1"}}}'
    Check ($init -match '"protocolVersion":"2025-06-18"') "el servidor MCP contesta a initialize"
    $mcp.StandardInput.WriteLine('{"jsonrpc":"2.0","method":"notifications/initialized"}')

    $copy = Join-Path $work "Anotalo-Setup-9.9.9.exe"
    Copy-Item -LiteralPath $Setup -Destination $copy -Force
    Set-Content -LiteralPath $copy -Stream Zone.Identifier -Value "[ZoneTransfer]", "ZoneId=3"
    Check ([bool](Get-Item -LiteralPath $copy -Stream Zone.Identifier -ErrorAction SilentlyContinue)) "el instalador lleva la marca de la web (ZoneId=3)"
    $p = Start-Exe $copy "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART" -Redirect
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(180000)) { $p.Kill(); Fail "el instalador no terminó en 3 min" }
    if ($err.Wait(10000) -and $out.Wait(1000)) {
        ($out.Result + $err.Result) -split "`r?`n" | Where-Object { $_ } | ForEach-Object { Write-Host "  | $_" }
    }
    else {
        # la app que abrió heredó la salida del instalador: un `| Out-Host` esperaría a que se cierre
        Write-Warning "la salida del instalador sigue abierta tras salir él (la heredó otro proceso)"
    }
    Check ($p.ExitCode -eq 0) "el instalador terminó con 0 (fue $($p.ExitCode))"
    Wait-All 60 ([ordered]@{
            "app.log: 'lo pide el instalador'"              = { (Read-Shared $appLog).Contains("lo pide el instalador") }
            "una app nueva (otro pid que $appPid)"          = { $a = @(Get-AppProcs); $a.Count -eq 1 -and $a[0].ProcessId -ne $appPid }
            "el servidor MCP sigue abierto"                 = { -not $mcp.HasExited }
            "el exe anterior apartado (Anotalo.exe.old-*)"  = { [bool](Get-ChildItem -LiteralPath $installDir -Filter "Anotalo.exe.old-*") }
            "el exe instalado no lleva la marca de la web"  = { -not (Get-Item -LiteralPath $exe -Stream Zone.Identifier -ErrorAction SilentlyContinue) }
        })
    $latest = Ask-Mcp '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_latest_call","arguments":{}}}'
    Check ($latest -match 'E2E-7Q' -and $latest -notmatch '"isError":true') "el servidor MCP de antes sigue leyendo la grabación"
    $mcp.StandardInput.Close()
    if (-not $mcp.WaitForExit(15000)) { Fail "el servidor MCP no terminó al cerrar su entrada" }
    Check ($mcp.ExitCode -eq 0) "el servidor MCP terminó con 0 (fue $($mcp.ExitCode))"
    if ($mcpErr.Wait(5000)) { $mcpErr.Result -split "`r?`n" | Where-Object { $_ } | ForEach-Object { Write-Host "  mcp | $_" } }

    Step 6 "desinstalar en silencio"
    Start-Sleep -Seconds 5 # la app nueva termina de arrancar (guarda sus ajustes al empezar)
    $cfgSha = Sha $cfg
    $code = Invoke-Native { & $exe --uninstall /S | Out-Host }
    Check ($code -eq 0) "el desinstalador terminó con 0 (fue $code)"
    Wait-All 30 ([ordered]@{
            "sin Programs\Anotalo"                          = { -not (Test-Path -LiteralPath $installDir) }
            "sin Anotalo.lnk en el menú Inicio"             = { -not (Test-Path -LiteralPath (Join-Path $programs "Anotalo.lnk")) }
            "sin Anotalo.lnk en Inicio"                     = { -not (Test-Path -LiteralPath (Join-Path $startup "Anotalo.lnk")) }
            "sin Anotalo.lnk en el escritorio"              = { -not (Test-Path -LiteralPath (Join-Path $desktop "Anotalo.lnk")) }
            "sin la clave de desinstalación de Anotalo"     = { -not (Test-Path -LiteralPath $anotaloKey) }
            "ningún Anotalo.exe abierto"                    = { -not (Get-Procs "Anotalo.exe") }
        })
    Check ((Test-Path -LiteralPath $cfg) -and (Sha $cfg) -eq $cfgSha) "los ajustes de Anotalo siguen ahí, sin cambios"
    Check ((Test-Path -LiteralPath $rec) -and (Sha $rec) -eq $recSha) "la grabación sigue ahí, sin cambios"
    Check ((Sha $legacyCfg) -eq $legacyCfgSha) "los ajustes de Transcriptor siguen ahí, sin cambios"

    $failed = $false
    Write-Host ""
    Write-Host "OK: cadena desde Transcriptor 0.6.4, actualización con la app abierta y desinstalación"
}
catch {
    Show-Diagnostics
    throw
}
finally {
    if ($mcp -and -not $mcp.HasExited) { $mcp.Kill() }
    if ($failed) {
        # lo que abrió esta prueba no debe dejar el job esperando
        foreach ($p in @(Get-Procs "Anotalo.exe") + @(Get-Procs "Transcriptor.exe")) {
            $path = "$($p.ExecutablePath)"
            if ($path -like "$installDir\*" -or $path -like "$innoDir\*") {
                Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
            }
        }
    }
}
