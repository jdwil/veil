# ProductHost static

Live product UI is **`ui/`** (Vite `:5180` in dev).

This directory only holds optional embed output:

- `viewer/` — unused hook for a same-origin static IDE (gitignored; product IDE is `ui/`)

The old generated dogfood SPA (`static/app`, `static/legacy`, `static/runtime-ui-gen`, `static/dist`) was removed. Do not resurrect `runtime.veil`. Set `VEIL_UI_DIR` to `ui/build` (or run Vite `:5180`) for the shell.
