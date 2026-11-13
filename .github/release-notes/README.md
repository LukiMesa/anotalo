# Notas de las versiones de Anotalo

Cada versión publicada tiene aquí sus notas, en español, en un archivo con el nombre de su etiqueta: `v1.0.0-alpha.3.md` para la etiqueta `v1.0.0-alpha.3`. Tiene que estar en `main` antes de ejecutar `scripts/release-anotalo.ps1`, que se niega sin él. El job `release` del workflow Anotalo lo usa tal cual como texto de la Release de GitHub y le añade el SHA-256 de `Anotalo-Setup-<versión>.exe` (`scripts/publish-release.ps1`).
