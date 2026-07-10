# VEIL dev server API

`veil serve` is the single HTTP surface for the IDE and agents.

## Entry point

```text
veil serve <file-or-dir> [-p PORT]
```

- CLI: `crates/veil-cli` → `veil_server::FilesystemProvider` + `veil_server::build_router`
- Implementation: `crates/veil-server` only (legacy `veil-cli/src/serve.rs` removed)
- Default port: `3001`
- CORS: permissive (local dev)

## Source of truth

On-disk `.veil` (and loaded layers/stubs) are authoritative. GET endpoints
re-parse and re-project IR/generated code; `POST /api/edit` applies structured
ops, re-serializes, checks, and writes back.

## Editability

`.veil` files are editable unless:

- path contains a `generated/` component, or
- source contains `# veil:readonly`

See `is_veil_source_editable` in `veil-cli`.

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/ir` | IR graph JSON for the active file |
| GET | `/api/source` | Raw active `.veil` source |
| GET | `/api/generated` | Generated code map (path → text) |
| GET | `/api/palette` | Construct + statement palette from layers |
| GET | `/api/presentation` | Layer presentation model (views, nest, layout) |
| GET | `/api/context` | Agent context pack (outline + presentation) |
| GET | `/api/stubs` | Loaded external crate stubs |
| GET | `/api/diagnostics` | Diagnostics array (compat; same pipeline as check) |
| GET/POST | `/api/check` | Full check pipeline (`?target=rust\|typescript`) |
| GET | `/api/files` | Loaded files (`index`, `name`, `path`, `editable`, `active`) |
| POST | `/api/files/select` | `{ "index": N }` — set active file |
| POST | `/api/edit` | `{ "ops": [ EditOp, … ] }` — structured edit |
| GET | `/api/diff` | Structural IR diff of active file vs git HEAD (UX-021) |

## Viewer assumptions

- Freeform canvas edges are **local only** (not persisted); graph edges are
  derived from IR (calls / refs / implements / sequence).
- Palette **Statements** and **External stubs** are reference browse, not
  inventable constructs (unless a layer construct says otherwise).
- Layer-provided infrastructure is **hidden by default**; when shown, dimmed
  and labeled `infra` (not re-serialized into user source).
