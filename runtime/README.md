# Runtime area — harness vs IDE vs platform (RT-006)

| Tool | Role |
|------|------|
| **`veil serve` in project root** | Single-product IDE (edit, check, agent) |
| **Runtime UX (local)** | Platform shell: configured **projects directory**, multi-project **tabs**, **IDE embedded** |
| **App `@main` harness** | How *this* app runs (`docs/HARNESS.md`) |
| **Local platform (fs+sqlite)** | Object store, compile pipeline, meta (RT-010+) |
| **Cloud adapters** | Provider-specific deploy (AWS/S3/DDB later) |

Project layout and modes: **[`docs/PROJECT_LAYOUT.md`](../docs/PROJECT_LAYOUT.md)**.

## Default story (RT-020)

**Single project (CLI / early IDE):**

```bash
# From one product repo (packages + layers/ + stubs/)
veil serve .
# open viewer → edit topology → check → agent prompt
```

**Runtime local (product direction):**

```bash
# Projects directory holds independent git repos (one product each)
export VEIL_PROJECTS_DIR=$HOME/dev/veil-projects   # default: ~/veil-projects
veil projects list
veil projects create my-app
# Runtime UX later: Open in IDE → spawn:
veil serve "$VEIL_PROJECTS_DIR/my-app" -p 3001
# or: make serve PROJECT=$VEIL_PROJECTS_DIR/my-app
```

- New projects: subdirectory + **git init** under `VEIL_PROJECTS_DIR`.
- Multi-product = **multiple IDE windows** (one `veil serve` per project), not tabs inside one IDE.
- `examples/` is demos/CI only (`make serve-examples`).

No special platform daemon is required for a single-project dual loop. Local
platform runtime is opt-in for multi-project shell, object storage, deploy, etc.

## Authoring your own harness

See **`docs/HARNESS.md`**: `@main` / `@pvd` / `@dep` composition, `veil gen`,
gaps (RT-001b bin layout, Bus declare, …).

## Env (agent + models)

See **`docs/AGENT.md`** and **`docs/SERVER.md`**.

## Local storage (RT-010 / RT-012)

```rust
// crates/veil-local
use veil_local::FsObjectStore;
let store = FsObjectStore::default_local()?; // ~/.veil/objects or VEIL_DATA_DIR
store.put("key", b"bytes")?;
let addr = store.put_addressed(b"blob")?; // sha256:…
```

Metadata sqlite store remains RT-011.

## Bootstrap

`runtime/bootstrap` is residual trampoline material — prefer VEIL-authored
`@main` (RT-000). Do not grow handwritten app registration as the only path.
