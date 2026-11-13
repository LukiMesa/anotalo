# Lanza la publicación de una versión de prueba de Anotalo (sin firmar): pone la versión en
# Cargo.toml y Cargo.lock, hace el commit "release: <versión>", la etiqueta v<versión> y sube las dos
# cosas. El workflow Anotalo compila esa etiqueta y, si pasan build, no-vulkan-smoke y setup-e2e, su
# job release publica Anotalo-Setup-<versión>.exe como pre-release (scripts/publish-release.ps1).
# Una versión estable se publica firmada (M5).
#
# Antes: las notas en .github/release-notes/v<versión>.md, ya en main y subidas.
# Uso: .\scripts\release-anotalo.ps1 -Version 1.0.0-alpha.3
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Repo = "LukiMesa/anotalo"
)
$ErrorActionPreference = "Stop"

$semver = '^[1-9][0-9]*\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$'
if ($Version -cnotmatch $semver) { throw "Versión no válida: $Version (p. ej. 1.0.0-alpha.3)" }
if (-not $Version.Contains("-")) { throw "Una versión estable se publica firmada (M5)." }
$tag = "v$Version"

# git y cargo escriben avisos y progreso por stderr: cuenta su código de salida, no eso (en Windows
# PowerShell, con la salida redirigida, cada línea de stderr sería un error que lo pararía todo).
function Invoke-Git {
    $ErrorActionPreference = "Continue"
    $out = & git @args
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') terminó con $LASTEXITCODE" }
    $out
}

function Invoke-Cargo {
    $ErrorActionPreference = "Continue"
    $out = & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo $($args -join ' ') terminó con $LASTEXITCODE" }
    $out
}

# [workspace.package] version = "<versión>" en Cargo.toml, sin tocar nada más del archivo.
function Set-WorkspaceVersion([string]$path, [string]$version) {
    $text = [IO.File]::ReadAllText($path)
    $nl = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
    $lines = $text -split "`r?`n"
    $section = $null
    $done = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*\[([^\]]+)\]\s*(#.*)?$') { $section = $Matches[1].Trim(); continue }
        if ($section -eq "workspace.package" -and -not $done -and $lines[$i] -match '^(\s*version\s*=\s*)"[^"]*"(.*)$') {
            $lines[$i] = $Matches[1] + '"' + $version + '"' + $Matches[2]
            $done = $true
        }
    }
    if (-not $done) { throw "No encontré version en [workspace.package] de $path" }
    [IO.File]::WriteAllText($path, ($lines -join $nl), (New-Object Text.UTF8Encoding $false))
}

# La ejecución del workflow para la etiqueta, si GitHub ya la muestra.
function Find-Run([string]$sha) {
    $ErrorActionPreference = "Continue"
    $json = gh run list -R $Repo --workflow anotalo.yml --commit $sha --json url,headBranch 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $json) { return $null }
    $runs = ($json | Out-String) | ConvertFrom-Json
    foreach ($r in $runs) { if ($r.headBranch -eq $tag) { return $r.url } }
    return $null
}

$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $branch = Invoke-Git rev-parse --abbrev-ref HEAD
    if ($branch -ne "main") { throw "Se publica desde main (estás en $branch)." }
    if (Invoke-Git status --porcelain) { throw "Hay cambios sin guardar en git." }
    Invoke-Git fetch origin
    $head = Invoke-Git rev-parse HEAD
    $remote = Invoke-Git rev-parse origin/main
    if ($head -ne $remote) { throw "main no coincide con origin/main ($head, $remote): haz pull o push antes." }
    if (Invoke-Git tag --list $tag) { throw "La etiqueta $tag ya existe." }
    if (Invoke-Git ls-remote --tags origin "refs/tags/$tag") { throw "La etiqueta $tag ya existe en origin." }
    $notes = ".github/release-notes/$tag.md"
    if (-not (Test-Path -LiteralPath $notes)) { throw "Faltan las notas: $notes" }

    Set-WorkspaceVersion (Join-Path $root "Cargo.toml") $Version
    $ok = $false
    try {
        Invoke-Cargo update --workspace --offline
        $changed = @(Invoke-Git status --porcelain | ForEach-Object { $_.Substring(3) })
        $other = @($changed | Where-Object { $_ -ne "Cargo.toml" -and $_ -ne "Cargo.lock" })
        if ($other) { throw "Además de Cargo.toml y Cargo.lock cambió: $($other -join ', ')" }
        $meta = (Invoke-Cargo metadata --no-deps --format-version 1 --offline | Out-String) | ConvertFrom-Json
        $now = @($meta.packages | Where-Object { $_.name -eq "anotalo" })[0].version
        if ($now -ne $Version) { throw "Cargo da la versión $now, no $Version" }
        $ok = $true
    }
    finally {
        if (-not $ok) { Invoke-Git restore Cargo.toml Cargo.lock }
    }

    Invoke-Git add Cargo.toml Cargo.lock
    Invoke-Git commit -q -m "release: $Version"
    Invoke-Git tag -a $tag -m "Anotalo $Version (prueba, sin firmar)"
    # las dos o ninguna: una etiqueta sin su commit en main no se publica
    Invoke-Git push --atomic origin main $tag
    $sha = Invoke-Git rev-parse HEAD

    $url = $null
    for ($i = 0; $i -lt 10 -and -not $url; $i++) {
        Start-Sleep -Seconds 3
        $url = Find-Run $sha
    }
    if ($url) { "Publicando ${tag}: $url" }
    else { "Publicando ${tag}: sigue la ejecución en https://github.com/$Repo/actions/workflows/anotalo.yml" }
    "Al terminar: gh release view $tag -R $Repo  (pre-release con un solo archivo, Anotalo-Setup-$Version.exe)"
}
finally {
    Pop-Location
}
