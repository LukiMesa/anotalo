# Descarga los modelos de los tests de Rust:
#   target\models\ggml-tiny.bin              (whisper.cpp)
#   target\models\nemo_en_titanet_small.onnx (voces, comprobado por SHA-256)
# Uso: .\scripts\get-test-model.ps1   -> imprime las rutas de los modelos
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$dir = Join-Path $root "target\models"
New-Item -ItemType Directory -Force $dir | Out-Null

function Get-Model($url, $name, $sha256) {
    $path = Join-Path $dir $name
    if (-not (Test-Path $path)) {
        $part = "$path.part"
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $part
        if ($sha256 -and (Get-FileHash -Algorithm SHA256 $part).Hash -ne $sha256.ToUpper()) {
            Remove-Item -Force $part
            throw "$name no coincide con el publicado (SHA-256)"
        }
        Move-Item -Force $part $path
    }
    Write-Output $path
}

Get-Model "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin" "ggml-tiny.bin" $null
Get-Model "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx" `
    "nemo_en_titanet_small.onnx" "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e"
