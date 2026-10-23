# Crea un acceso directo en la carpeta Inicio para arrancar el transcriptor con Windows.
$root = Split-Path $PSScriptRoot -Parent
$startup = [Environment]::GetFolderPath("Startup")
$lnk = Join-Path $startup "Transcriptor de llamadas.lnk"
$sh = New-Object -ComObject WScript.Shell
$s = $sh.CreateShortcut($lnk)
$s.TargetPath = Join-Path $root ".venv\Scripts\pythonw.exe"
$s.Arguments = "`"$(Join-Path $root 'run.pyw')`" --minimized"
$s.WorkingDirectory = $root
$s.Save()
Write-Host "Acceso directo creado: $lnk"
