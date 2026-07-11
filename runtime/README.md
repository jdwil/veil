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
# First run writes ~/.veil/config.json (projects_dir, etc.)
make runtime-serve          # or: cargo run --release -p veil-runtime-bootstrap
# multi IDE kernel + bus on :8080
# Viewer (separate Vite in dev):
#   http://127.0.0.1:5173/?project=<name>
```

- Config: `~/.veil/config.json` (`projects_dir`); env `VEIL_PROJECTS_DIR` overrides.
- **`veil-runtime` embeds `veil-server::build_multi_router`** (same routes as `veil serve --multi`).
- CLI convenience: `veil serve --multi -p 3001` or `veil serve <project>`.
- Viewer multi: `?project=<name>` against runtime `:8080` or multi serve.
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
