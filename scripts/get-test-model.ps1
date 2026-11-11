# Descarga el modelo pequeño de whisper.cpp para los tests de Rust (target\models\ggml-tiny.bin).
# Uso: .\scripts\get-test-model.ps1   -> imprime la ruta del modelo
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$dir = Join-Path $root "target\models"
$path = Join-Path $dir "ggml-tiny.bin"
if (-not (Test-Path $path)) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    $part = "$path.part"
    Invoke-WebRequest -UseBasicParsing -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin" -OutFile $part
    Move-Item -Force $part $path
}
Write-Output $path
