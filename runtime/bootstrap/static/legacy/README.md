# Quarantined handwritten shell (PVR-032)

Handwritten product HTML used to live as the primary runtime shell.
**Do not use as the product UI.** Author `runtime/src/runtime-ui.veil` instead.

Primary path after `make pure-runtime-build`:

- `../dist/index.html` + `../dist/spa.js` (generated, CAP-005)
- Host: `veil_server::ProductHost` serves `dist/` at `GET /`

`ide.html` remains for iframe embed of the dual-loop viewer until the viewer
is fully same-origin bundled.
