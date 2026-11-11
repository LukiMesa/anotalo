# Transcriptor de llamadas

Transcribe en local y en tiempo real tus llamadas (Teams, Zoom, WhatsApp, Slack, Discord o cualquier web en el navegador) y tus **notas rápidas** dictadas con un atajo.
En las llamadas, tu voz sale como **Yo** y la de los demás como **Otros**. Todo se guarda en Markdown, listo para leer o pasárselo a un asistente.

- 100 % local: Whisper `large-v3-turbo` (faster-whisper) en tu GPU NVIDIA, o `small` con el procesador. Nada sale del PC.
- Detecta las llamadas solo y también tiene un botón **Grabar llamada**.
- Se actualiza desde la propia app.
- Arranca con Windows y vive junto al reloj.
- Ventana con todas las grabaciones: buscar, copiar, descargar `.md` y borrar (con "Deshacer").
- Notas rápidas con `Ctrl+Alt+R`: al parar, el texto se copia al portapapeles.
- Indicador pequeño abajo en el centro mientras se graba.
- Tema oscuro propio con Sora / Instrument Sans / JetBrains Mono.

## Instalación

1. Descarga `Transcriptor-Setup-x.y.z.exe` desde **Releases** de este repositorio.
2. Ejecútalo. Se instala en tu usuario, sin permisos de administrador, con acceso en el menú Inicio y la opción de iniciar con Windows.
3. Si Windows muestra **"Windows protegió tu PC"**: *Más información* → *Ejecutar de todas formas* (el instalador no está firmado).
4. La primera vez la app se prepara (la barra superior muestra *Preparando… NN %*); hasta que termine no graba:
   - **PC con NVIDIA**: descarga los componentes de GPU (~1,3 GB) y el modelo `large-v3-turbo`.
   - **PC sin NVIDIA**: usa el procesador con el modelo `small` (más ligero, en tiempo real).

Requisitos: Windows 10/11 de 64 bits y Microsoft Edge WebView2 (viene con Windows 11; el instalador avisa si falta). Desinstalar no borra tus grabaciones.

### Actualizar

Desde la versión 0.6.0 la app comprueba si hay una versión nueva al arrancar y cada 6 horas (y con **General → Buscar actualizaciones**). Si la hay, aparece un aviso con **Actualizar**: descarga el instalador, comprueba que es el publicado, se instala sola y vuelve a abrirse. Nunca se instala durante una grabación ni sin tu clic. Se conservan ajustes y grabaciones.

Las versiones anteriores a 0.6.0 se actualizan instalando la nueva encima, a mano, una vez.

## Uso

La ventana tiene su propia barra superior: arrástrala para mover, doble clic para maximizar, bordes para redimensionar; la **×** la oculta y la app sigue junto al reloj.

- **Abrir la ventana**: clic en el icono junto al reloj, o vuelve a abrir la app.
- **Nota rápida**: `Ctrl+Alt+R` para empezar y otra vez para parar. Graba solo tu micrófono y copia al portapapeles lo que dijiste (sin horas ni etiquetas).
- **Llamadas automáticas**: se graban solas cuando una de estas apps usa el micrófono, y paran al colgar:
  - Teams, Zoom, Slack y Discord (Discord viene desactivado: también se usa fuera de llamadas de trabajo). En Teams se graba lo que suena en la salida que usa (sus auriculares, aunque sean Bluetooth).
  - WhatsApp: cuenta como llamada tras 30 s seguidos de micrófono, para no grabar notas de voz cortas (los primeros 30 s no se transcriben).
  - Navegadores (Firefox, Chrome, Brave, Edge, Opera, Vivaldi): cualquier web que use el micrófono. La grabación toma el nombre de la plataforma si se ve en el título (Meet, Teams, Zoom, WhatsApp…), si no el del navegador.
  - Puedes cambiar de pestaña o de app sin cortar la grabación. Si paras una grabación a mano, esa app no vuelve a grabar hasta que suelte el micrófono.
- **Grabar llamada** (botón junto a "Grabaciones" o menú del icono): graba tu micrófono y lo que suena en la salida donde está la llamada (pensado para auriculares). Para con el mismo botón o con el atajo.
- Los dispositivos se eligen al empezar cada grabación: si conectas unos auriculares (también Bluetooth), se usan los que usa la llamada.
- Las llamadas se guardan, no se copian. El atajo durante una llamada la detiene.
- **Indicador**: píldora con barras al ritmo de tu voz → "Transcribiendo…" → "Copiado al portapapeles".

### Ventana

- **Grabaciones**: agrupadas por día, filtro Todas / Llamadas / Notas rápidas y buscador (sin importar tildes).
  - *Copiar*: texto con horas y hablante (`[00:00:05] Yo: …`).
  - *Descargar .md*: el archivo tal cual.
  - *Borrar*: se puede deshacer durante unos segundos.
- **Atajo y grabación**: cambia el atajo (pulsa el lápiz y la nueva combinación; Esc cancela), copiar al terminar, mostrar indicador.
- **Llamadas**: detección automática y cada plataforma por separado; etiquetas Yo / Otros.
- **Modelo e idioma**: modelo Whisper e idioma (se recarga en segundo plano).
- **General**: iniciar con Windows, carpeta de grabaciones y actualizaciones.

Las grabaciones se guardan en `Documentos\Llamadas\AAAA-MM-DD_HHMM_<App>.md` (`<App>` = Teams, Meet, Zoom…, `Llamada` para las grabadas a mano o `Nota`). Ajustes en `%APPDATA%\CallTranscriber\config.json`; registro en `%APPDATA%\CallTranscriber\app.log`.

## Notas

- En las llamadas se transcribe todo lo que entra por tu micrófono, aunque estés silenciado.
- Pensado para usar con auriculares.
- Prueba de captura: `.\.venv\Scripts\python scripts\probe_audio.py --process firefox.exe`
- Demo del indicador: `.\.venv\Scripts\python scripts\demo_indicator.py`

## Desarrollo (desde el código)

```powershell
.\setup.ps1
.\.venv\Scripts\pythonw.exe run.pyw
.\.venv\Scripts\pytest -q
```

La versión de desarrollo usa sus propios ajustes (`%APPDATA%\CallTranscriber-dev`), guarda en `Documentos\Llamadas (dev)`, no toca el arranque con Windows ni se actualiza sola, así que puede convivir con la instalada.

### Crear una versión nueva

```powershell
.\scripts\build.ps1     # tests + ejecutable (PyInstaller) + instalador (Inno Setup) -> dist\
.\scripts\release.ps1   # publica dist\Transcriptor-Setup-<versión>.exe como Release en GitHub
```

La versión sale de `calltx/__init__.py`. Requiere Inno Setup 6 (`winget install JRSoftware.InnoSetup`) y `gh` con sesión iniciada. Las apps instaladas ven la nueva versión en cuanto se publica la Release.

## Code signing policy

Free code signing provided by SignPath.io, certificate by SignPath Foundation

Las versiones firmadas serán las de **Anotalo**, la reescritura de esta app que está en marcha, compiladas en GitHub Actions a partir de este repositorio.

- Committers and reviewers: [LukiMesa](https://github.com/LukiMesa)
- Approvers: [LukiMesa](https://github.com/LukiMesa)

Cada versión firmada se aprueba a mano antes de firmarse. Qué datos usa la app: [política de privacidad](PRIVACY.md).

Licencia MIT. Fuentes Sora, Instrument Sans y JetBrains Mono bajo SIL Open Font License (ver `calltx/ui/web/fonts/`).
