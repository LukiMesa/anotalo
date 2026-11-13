# Política de privacidad

**Transcriptor** y **Anotalo** (su reescritura, ahora en versión de prueba) graban y transcriben en tu PC. Esta página dice exactamente qué sale del equipo.

## Lo que nunca sale de tu PC

- Sin telemetría, sin cuentas, sin estadísticas de uso.
- Las grabaciones y las transcripciones se guardan en tu PC (por defecto en `Documentos\Llamadas`) y la app no las envía a ningún sitio.

## Conexiones que hace la app

Todas son anónimas: no se envía ningún dato tuyo ni de tus llamadas.

| Servicio | Para qué | Cuándo |
|---|---|---|
| `api.github.com` y `github.com` | Comprobar si hay una versión nueva y descargarla | Al abrir la app, cada 6 horas y al pulsar "Buscar actualizaciones" |
| `huggingface.co` | Descargar el modelo de transcripción | La primera vez, o al cambiar de modelo |
| `pypi.org` y `files.pythonhosted.org` | Descargar los componentes de GPU de NVIDIA | Solo la versión actual (Transcriptor) y solo en PCs con tarjeta NVIDIA, la primera vez |

## Anotalo

- Las actualizaciones se buscan solas al abrir la app y cada 6 horas; puedes desactivarlo en **Ajustes › General › Buscar actualizaciones automáticamente**. Una versión de prueba también se actualiza a versiones de prueba más nuevas.
- No usa PyPI: no descarga componentes de GPU.
- Si pones nombre a una persona en una grabación, guarda su **huella de voz** para reconocerla en próximas llamadas. Se guarda solo en tu PC, en `%APPDATA%\Anotalo\voces.json`, y puedes borrarla desde **Ajustes › Voces guardadas**.
- Para poder nombrar a alguien después de colgar, cada llamada guarda las huellas de voz de sus participantes durante **30 días** en `%APPDATA%\Anotalo\llamadas`; pasado ese tiempo se borran solas. Nunca salen de tu PC.
- Descarga una vez el modelo que separa las voces (40 MB) de GitHub, de la publicación de [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx); como en las demás descargas, GitHub ve tu dirección IP.
- **"Copiar diagnóstico"** (Ajustes › General) solo copia al portapapeles la versión, los ajustes, el audio en uso y el final del registro, sin títulos de ventanas ni nada de lo dicho en las llamadas. Tú decides si lo mandas.

### Claude Code

- Si conectas Anotalo con [Claude Code](https://claude.com/claude-code) (**Ajustes › General › Conectar con Claude Code**), Claude Code puede leer tus transcripciones **cuando se lo pides**. Lo que lee pasa a formar parte de tu conversación con Claude y se rige por la política de Claude Code.
- Anotalo no envía nada por su cuenta. Su servidor para Claude Code (`anotalo mcp`) solo lee la carpeta de grabaciones: no escribe, no borra y no se conecta a internet.
- **"Copiar para Claude"** solo copia la transcripción al portapapeles; pegarla en algún sitio es cosa tuya.
- Para desconectarlo: `claude mcp remove --scope user anotalo`.

## Políticas de los servicios que usa

- GitHub: [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- Hugging Face: [Privacy Policy](https://huggingface.co/privacy)
- PyPI: [Privacy Notice](https://policies.python.org/pypi.org/Privacy-Notice/)
- Claude Code: [Anthropic Privacy Policy](https://www.anthropic.com/legal/privacy)

---

# Privacy policy (English)

**Transcriptor** and **Anotalo** (its rewrite, now in a test version) record and transcribe on your PC. This page states exactly what leaves the computer.

## What never leaves your PC

- No telemetry, no accounts, no usage statistics.
- Recordings and transcripts are stored on your PC (by default in `Documents\Llamadas`) and the app never sends them anywhere.

## Network connections the app makes

All of them are anonymous: no data about you or your calls is sent.

| Service | Purpose | When |
|---|---|---|
| `api.github.com` and `github.com` | Check for a new version and download it | At start-up, every 6 hours, and on "Buscar actualizaciones" |
| `huggingface.co` | Download the transcription model | The first time, or when the model changes |
| `pypi.org` and `files.pythonhosted.org` | Download NVIDIA GPU components | Current version (Transcriptor) only, only on PCs with an NVIDIA card, the first time |

## Anotalo

- It checks for updates at start-up and every 6 hours; you can turn that off in **Ajustes › General › Buscar actualizaciones automáticamente**. A test version also updates to newer test versions.
- It does not use PyPI: it downloads no GPU components.
- If you name a person in a recording, it stores their **voice print** to recognise them in later calls. It is stored only on your PC, in `%APPDATA%\Anotalo\voces.json`, and can be deleted from **Ajustes › Voces guardadas**.
- So you can name someone after hanging up, each call keeps its participants' voice prints for **30 days** in `%APPDATA%\Anotalo\llamadas`; after that they are deleted automatically. They never leave your PC.
- It downloads the voice-separation model (40 MB) once from GitHub, from the [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) release; as with the other downloads, GitHub sees your IP address.
- **"Copiar diagnóstico"** (Ajustes › General) only copies to the clipboard the version, the settings, the audio in use and the end of the log, without window titles or anything said in calls. You decide whether to send it.

### Claude Code

- If you connect Anotalo with [Claude Code](https://claude.com/claude-code) (**Ajustes › General › Conectar con Claude Code**), Claude Code can read your transcripts **when you ask it to**. What it reads becomes part of your conversation with Claude, under Claude Code's own policy.
- Anotalo sends nothing by itself. Its server for Claude Code (`anotalo mcp`) only reads the recordings folder: it does not write, delete or connect to the internet.
- **"Copiar para Claude"** only copies the transcript to the clipboard; pasting it anywhere is up to you.
- To disconnect it: `claude mcp remove --scope user anotalo`.

## Policies of the services it uses

- GitHub: [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- Hugging Face: [Privacy Policy](https://huggingface.co/privacy)
- PyPI: [Privacy Notice](https://policies.python.org/pypi.org/Privacy-Notice/)
- Claude Code: [Anthropic Privacy Policy](https://www.anthropic.com/legal/privacy)
