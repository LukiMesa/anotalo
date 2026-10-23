# Transcriptor de llamadas

Transcribe en local y en tiempo real tus llamadas de **Microsoft Teams** y **Google Meet** (Firefox).
Tu voz sale como **Yo** y la de los demás como **Otros**, en un Markdown por llamada listo para leer
o pasárselo a Claude y sacar la lista de cambios que pide el cliente.

- 100 % local: Whisper `large-v3-turbo` (faster-whisper) en tu GPU NVIDIA. Nada sale del PC.
- "Otros" captura **solo el audio de Teams o Firefox** (no notificaciones ni otras apps).
- Arranque automático al detectar una llamada, o manual con `Ctrl+Alt+R`.
- Puedes cambiar de pestaña o de app durante la llamada: la grabación solo termina cuando cuelgas.

## Instalación

Requisitos: Windows 11, Python 3.13, GPU NVIDIA con driver reciente (sin GPU funciona en CPU con un modelo más pequeño).

```powershell
.\setup.ps1
```

La primera vez descarga el modelo (~1,6 GB) desde Hugging Face; después funciona sin conexión.

## Uso

```powershell
.\.venv\Scripts\pythonw.exe run.pyw
```

Aparece un icono de micrófono junto al reloj:

| Color | Estado |
|------|--------|
| Gris | Inactivo (modo automático apagado) |
| Azul | Esperando llamada |
| Rojo | Grabando |

- **Ctrl+Alt+R**: inicia / detiene la grabación.
- **Modo automático**: empieza cuando Teams usa el micrófono, o cuando Firefox lo usa con una ventana de Meet; para cuando la app suelta el micrófono.
- **Ver transcripción en vivo**: ventana con el texto según se habla y botón *Copiar todo*.
- Las transcripciones se guardan en `Documentos\Llamadas\AAAA-MM-DD_HHMM_<App>.md`, línea a línea (si algo falla, lo ya dicho queda guardado).

Para arrancar con Windows:

```powershell
.\scripts\install_autostart.ps1
```

## Ajustes

`%APPDATA%\CallTranscriber\config.json` (se crea al primer arranque; reinicia la app tras editarlo):

| Clave | Por defecto | Qué hace |
|------|------|------|
| `model` | `large-v3-turbo` | Modelo Whisper |
| `language` | `es` | Idioma de las llamadas |
| `device` / `compute_type` | `cuda` / `float16` | GPU; usa `cpu` / `int8` sin GPU |
| `label_me` / `label_others` | `Yo` / `Otros` | Etiquetas |
| `hotkey` | `<ctrl>+<alt>+r` | Atajo global |
| `auto_mode` | `true` | Detección automática de llamadas |
| `output_dir` | `Documentos\Llamadas` | Carpeta de salida |
| `mic_device` | `null` | Parte del nombre del micro (si no, el predeterminado de Windows) |
| `silence_ms` | `600` | Pausa que cierra una frase |

Registro de la app: `%APPDATA%\CallTranscriber\app.log`.

## Notas

- Se transcribe todo lo que entra por tu micrófono, aunque estés silenciado en la llamada.
- Pensado para usar con auriculares (con altavoces, el micro también capta al cliente).
- Prueba de captura: `.\.venv\Scripts\python scripts\probe_audio.py --process firefox.exe`

## Desarrollo

```powershell
.\.venv\Scripts\pytest -q
```

Licencia MIT.
