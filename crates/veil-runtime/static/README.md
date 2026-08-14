# ProductHost static

Live product UI is **`ui/`** (Vite `:5180` in dev).

This directory only holds optional embed output:

- `viewer/` — unused hook for a same-origin static IDE (gitignored; product IDE is `ui/`)
- `dist/` — optional bundled shell for `GET /` when Vite is not in front (gitignored)

The old generated dogfood SPA (`static/app`, `static/legacy`, `static/runtime-ui-gen`) was removed. Do not resurrect `runtime.veil`.
