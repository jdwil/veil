# Generated shell output (CAP-005)

`make pure-runtime-build` runs:

```bash
veil gen runtime/src/runtime-ui.veil -o runtime/bootstrap/static/app -t typescript
```

Primary product UI is copied to **`../dist/`** (`index.html` + `spa.js`).
`ProductHost` serves `static/dist/` first at `GET /`.

Do not hand-edit product HTML here — author `runtime/src/runtime-ui.veil`.
