# Transcriptor de llamadas

Transcribe en local y en tiempo real tus llamadas de **Microsoft Teams** y **Google Meet** (Firefox), y tus **notas rápidas** dictadas con un atajo.
En las llamadas, tu voz sale como **Yo** y la de los demás como **Otros**. Todo se guarda en Markdown, listo para leer o pasárselo a Claude.

- 100 % local: Whisper `large-v3-turbo` (faster-whisper) en tu GPU NVIDIA. Nada sale del PC.
- Arranca con Windows y vive junto al reloj.
- Ventana con todas las grabaciones: buscar, copiar, descargar `.md` y borrar (con "Deshacer").
- Notas rápidas con `Ctrl+Alt+R`: al parar, el texto se copia al portapapeles.
- Indicador pequeño abajo en el centro mientras se graba.
- Diseño con un sistema propio (tema Noche, Sora / Instrument Sans / JetBrains Mono).

## Instalación

Requisitos: Windows 11, Python 3.13, GPU NVIDIA con driver reciente (sin GPU funciona en CPU con un modelo más pequeño). WebView2 viene con Windows 11.

```powershell
.\setup.ps1
```

La primera vez descarga el modelo (~1,6 GB) desde Hugging Face; después funciona sin conexión.

## Uso

```powershell
.\.venv\Scripts\pythonw.exe run.pyw
```

La primera vez se activa el **arranque con Windows** (minimizado junto al reloj). Puedes desactivarlo en *Ajustes › General*.

- **Abrir la ventana**: clic en el icono junto al reloj, o vuelve a abrir la app.
- **Nota rápida**: `Ctrl+Alt+R` para empezar y otra vez para parar. Graba solo tu micrófono y copia al portapapeles lo que dijiste (sin horas ni etiquetas).
- **Llamadas**: se graban solas cuando Teams usa el micrófono, o cuando Firefox lo usa con Google Meet abierto; paran al colgar. Puedes cambiar de pestaña o de app sin cortar la grabación. Las llamadas se guardan, no se copian. El atajo durante una llamada la detiene.
- **Indicador**: píldora con barras al ritmo de tu voz → "Transcribiendo…" → "Copiado al portapapeles".

### Ventana

- **Grabaciones**: agrupadas por día, filtro Todas / Llamadas / Notas rápidas y buscador (sin importar tildes).
  - *Copiar*: texto con horas y hablante (`[00:00:05] Yo: …`).
  - *Descargar .md*: el archivo tal cual.
  - *Borrar*: se puede deshacer durante unos segundos.
- **Atajo y grabación**: cambia el atajo (pulsa el lápiz y la nueva combinación; Esc cancela), copiar al terminar, mostrar indicador.
- **Llamadas**: detección automática, Teams / Meet por separado, etiquetas Yo / Otros.
- **Modelo e idioma**: modelo Whisper e idioma (se recarga en segundo plano).
- **General**: iniciar con Windows y carpeta de grabaciones.

Las grabaciones se guardan en `Documentos\Llamadas\AAAA-MM-DD_HHMM_<App>.md` (`<App>` = Teams, Meet o Nota). Ajustes en `%APPDATA%\CallTranscriber\config.json`; registro en `%APPDATA%\CallTranscriber\app.log`.

## Notas

- En las llamadas se transcribe todo lo que entra por tu micrófono, aunque estés silenciado.
- Pensado para usar con auriculares.
- Prueba de captura: `.\.venv\Scripts\python scripts\probe_audio.py --process firefox.exe`
- Demo del indicador: `.\.venv\Scripts\python scripts\demo_indicator.py`

## Desarrollo

```powershell
.\.venv\Scripts\pytest -q
```

Licencia MIT. Fuentes Sora, Instrument Sans y JetBrains Mono bajo SIL Open Font License (ver `calltx/ui/web/fonts/`).
