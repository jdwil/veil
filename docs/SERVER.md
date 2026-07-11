# VEIL dev server API

`veil serve` is the single HTTP surface for the IDE and agents.

Project roots, multi-project runtime tabs, and layer visibility:
**[`PROJECT_LAYOUT.md`](PROJECT_LAYOUT.md)**.

## Entry point

```text
veil serve <file-or-dir> [-p PORT]
```

- CLI: `crates/veil-cli` → `veil_server::FilesystemProvider` + `veil_server::build_router`
- Implementation: `crates/veil-server` only (legacy `veil-cli/src/serve.rs` removed)
- Default port: `3001`
- CORS: permissive (local dev)
- Prefer a **single project root** (packages + `layers/` + `stubs/`). `examples/`
  is for demos/CI, not the product default workspace.

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
| POST | `/api/source` | Full-file write (parse+check first; AGT-010 remote) |
| GET | `/api/generated` | Generated code map (path → text) |
| GET | `/api/palette` | Construct + statement palette from layers |
| GET | `/api/presentation` | Layer presentation model (views, nest, layout) |
| GET | `/api/context` | Agent context pack (outline + presentation) |
| GET | `/api/stubs` | Loaded external crate stubs |
| GET | `/api/diagnostics` | Diagnostics array (compat; same pipeline as check) |
| GET/POST | `/api/check` | Full check pipeline (`?target=rust\|typescript\|swift\|kotlin`) |
| GET | `/api/files` | Loaded files (`index`, `name`, `path`, `editable`, `active`) |
| POST | `/api/files/select` | `{ "index": N }` — set active file |
| GET | `/api/project` | Active IDE project `{ name, path, projects_dir }` |
| GET | `/api/projects` | Hub: products under configured projects dir |
| POST | `/api/projects` | Hub: create product `{ "name" }` (git + scaffold) |
| GET | `/api/config` | Public subset of `~/.veil/config.json` |
| * | `/api/p/{project}/…` | Multi-project: same IDE routes scoped to a product (`veil serve --multi`) |

Multi-project: `veil serve --multi` → hub `/api/projects` + per-project
`/api/p/{name}/ir` etc. Viewer: `?project=name`. See [`IDE_RUNTIME.md`](IDE_RUNTIME.md).
| POST | `/api/edit` | `{ "ops": [ EditOp, … ] }` — structured edit |
| GET | `/api/diff` | Structural IR diff of active file vs git HEAD (UX-021) |
| POST | `/api/agent/turn` | Built-in agent turn `{ "prompt": "…" }` (Rig or heuristic) |
| GET | `/api/agent/tools` | Rig tool JSON schemas (MCP bridge discovery) |
| GET | `/api/models` | Configured model provider (Rig) |
| GET | `/api/events` | SSE revision heartbeat for live sync (AGT-002) |

## Remote SourceStore (AGT-010)

```bash
# On host with package files:
veil serve path/to/pkg.veil -p 3001

# On IDE/agent machine (no LocalFs for package):
VEIL_REMOTE_URL=http://host:3001 veil serve -p 3002
```

`RemoteHttpProvider` (reqwest, no curl) proxies:

- `/api/files`, `/api/files/select`
- `/api/source` GET/POST
- `/api/edit` (structured EditOps — AGT-017 `forward_edit`)

Layers still resolve from the client registry / layer path.

### Live sync (AGT-018)

- Proxy `GET /api/events` returns revision from **remote** source content hash.
- SSE `data` may include `remote_events` (host URL) so the IDE can subscribe
  directly to the host stream when tunnels allow.
- Fallback: poll `GET /api/events` or re-fetch IR after agent `source_changed`.

Authn for multi-user cloud: optional `VEIL_AUTH_TOKEN` (AGT-016) or reverse-proxy
in front of serve.

## Viewer assumptions

- Freeform canvas edges are **local only** (not persisted); graph edges are
  derived from IR (calls / refs / implements / sequence).
- Palette **Statements** and **External stubs** are reference browse, not
  inventable constructs (unless a layer construct says otherwise).
- Layer-provided infrastructure is **hidden by default**; when shown, dimmed
  and labeled `infra` (not re-serialized into user source).
