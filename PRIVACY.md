# Política de privacidad

**Transcriptor** (y **Anotalo**, su reescritura en marcha) graba y transcribe en tu PC. Esta página dice exactamente qué sale del equipo.

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

## Anotalo (próximamente)

- Añade el ajuste **"Buscar actualizaciones automáticamente"** para desactivar la comprobación periódica.
- Deja de usar PyPI: no descarga componentes de GPU.
- Si pones nombre a una persona en una grabación, guarda su **huella de voz** para reconocerla en próximas llamadas. Se guarda solo en tu PC, en `%APPDATA%\Anotalo\voces.json`, y puedes borrarla desde Ajustes.

## Políticas de los servicios que usa

- GitHub: [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- Hugging Face: [Privacy Policy](https://huggingface.co/privacy)
- PyPI: [Privacy Notice](https://policies.python.org/pypi.org/Privacy-Notice/)

---

# Privacy policy (English)

**Transcriptor** (and **Anotalo**, its rewrite in progress) records and transcribes on your PC. This page states exactly what leaves the computer.

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

## Anotalo (coming)

- Adds the setting **"Buscar actualizaciones automáticamente"** to turn off the periodic check.
- Stops using PyPI: it downloads no GPU components.
- If you name a person in a recording, it stores their **voice print** to recognise them in later calls. It is stored only on your PC, in `%APPDATA%\Anotalo\voces.json`, and can be deleted from Ajustes.

## Policies of the services it uses

- GitHub: [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- Hugging Face: [Privacy Policy](https://huggingface.co/privacy)
- PyPI: [Privacy Notice](https://policies.python.org/pypi.org/Privacy-Notice/)
