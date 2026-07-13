# Quarantined handwritten shell (PVR-032)

**Not served by the default product host.** Do not reintroduce these as
primary UI.

| File | Former role |
|------|-------------|
| `ide.html` | Iframe chrome around dual-loop viewer |
| `index.redirect.html` | Meta-refresh stub |

## Primary path (required)

After `make pure-runtime-build`:

- `../dist/index.html` + `../dist/spa.js` — generated from `runtime/src/runtime-ui.veil`
- `../viewer/` — built `veil-viewer` at `/viewer`
- IDE open: **redirect** to `/viewer/?project=…` (not iframe)

`ProductHost` serves **only** `dist/index.html` for `GET /` and SPA fallbacks.
CI / `scripts/pure_runtime_smoke.sh` fails if `static/ide.html` reappears at the
static root or if Rust host sources reference `ide.html`.
