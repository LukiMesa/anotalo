# Publica una versión de prueba de Anotalo, sin firmar, como pre-release de GitHub: un solo archivo,
# Anotalo-Setup-<versión>.exe, y las notas de .github/release-notes/<etiqueta>.md con su SHA-256.
# Una pre-release nunca es /releases/latest, así que Transcriptor 0.6.x no la ve. Una versión estable
# se publica firmada (M5).
#
# Uso (lo ejecuta el job release del workflow Anotalo, con GH_TOKEN):
#   .\scripts\publish-release.ps1 -Exe dist\anotalo.exe -Tag v1.0.0-alpha.3
# Comprobar sin publicar (enseña el título, las notas y el archivo; sin notas también vale):
#   .\scripts\publish-release.ps1 -Exe target\release\anotalo.exe -Tag v1.0.0-alpha.3 -DryRun
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Tag,
    [string]$Repo = $(if ($env:GITHUB_REPOSITORY) { $env:GITHUB_REPOSITORY } else { "LukiMesa/anotalo" }),
    [switch]$DryRun
)
$ErrorActionPreference = "Stop"

$semver = '^[1-9][0-9]*\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$'
if ($Tag -cnotmatch '^v') { throw "La etiqueta debe empezar por v: $Tag" }
$version = $Tag.Substring(1)
if ($version -cnotmatch $semver) { throw "Versión no válida: $version (p. ej. v1.0.0-alpha.3)" }
if (-not $version.Contains("-")) { throw "Una versión estable se publica firmada (M5)." }

$root = Split-Path $PSScriptRoot -Parent
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$name = "Anotalo-Setup-$version.exe"
$out = Join-Path ([IO.Path]::GetTempPath()) "anotalo-release-$version"
New-Item -ItemType Directory -Force $out | Out-Null
$asset = Join-Path $out $name
Copy-Item -LiteralPath $exePath -Destination $asset -Force
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $asset).Hash.ToLowerInvariant()

$notesSrc = Join-Path $root ".github\release-notes\$Tag.md"
if (Test-Path -LiteralPath $notesSrc) {
    $notes = [IO.File]::ReadAllText($notesSrc).Replace("`r`n", "`n").TrimEnd()
}
elseif ($DryRun) {
    Write-Warning "Faltan las notas .github/release-notes/$Tag.md (sin -DryRun, no se publicaría)"
    $notes = "(faltan las notas: .github/release-notes/$Tag.md)"
}
else {
    throw "Faltan las notas: .github/release-notes/$Tag.md"
}
$notes += "`n`nSHA-256 de ``$name``: ``$hash```n"
$title = "Anotalo $version (prueba, sin firmar)"

"Repositorio: $Repo"
"Etiqueta: $Tag (pre-release, nunca la última)"
"Título: $title"
"Archivo: $asset ($((Get-Item -LiteralPath $asset).Length) bytes)"
"SHA-256: $hash"
"Notas:"
$notes
if ($DryRun) {
    Remove-Item -LiteralPath $out -Recurse -Force
    "-DryRun: no se publica nada."
    return
}

# gh escribe progreso por stderr: cuenta su código de salida.
function Invoke-Gh {
    $ErrorActionPreference = "Continue"
    $res = & gh @args
    if ($LASTEXITCODE -ne 0) { throw "gh $($args[0]) $($args[1]) terminó con $LASTEXITCODE" }
    $res
}

# Las notas van en un archivo: así no pasan por la línea de comandos.
$notesFile = Join-Path $out "notas.md"
[IO.File]::WriteAllText($notesFile, $notes, (New-Object Text.UTF8Encoding $false))
# --verify-tag: la etiqueta tiene que existir ya (la sube release-anotalo.ps1); gh no crea otra.
Invoke-Gh release create $Tag $asset -R $Repo --verify-tag --prerelease --latest=false --title $title --notes-file $notesFile

# Lo que protege a Transcriptor 0.6.x y a quien descarga: comprobado en GitHub, no supuesto.
$latest = Invoke-Gh api "repos/$Repo/releases/latest" --jq .tag_name
if ($latest -eq $Tag) { throw "$Tag ha quedado como /releases/latest: Transcriptor 0.6.x la vería" }
"/releases/latest sigue siendo $latest"
$pre = Invoke-Gh release view $Tag -R $Repo --json isPrerelease --jq .isPrerelease
if ($pre -ne "true") { throw "$Tag no ha quedado como pre-release" }
$digest = $null
for ($i = 0; $i -lt 10; $i++) {
    $assets = @(((Invoke-Gh release view $Tag -R $Repo --json assets) | Out-String | ConvertFrom-Json).assets)
    if ($assets.Count -ne 1 -or $assets[0].name -ne $name) {
        throw "$Tag debe tener solo $name; tiene: $(($assets | ForEach-Object { $_.name }) -join ', ')"
    }
    $digest = $assets[0].digest
    if ($digest) { break }
    Start-Sleep -Seconds 3
}
if ($digest -ne "sha256:$hash") { throw "El SHA-256 de $name en GitHub ($digest) no es el subido (sha256:$hash)" }
"Publicada: https://github.com/$Repo/releases/tag/$Tag"
