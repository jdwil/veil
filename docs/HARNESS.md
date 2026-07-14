# Authoring your own app harness (RT-000)

VEIL’s daily path is **VEIL-authored harnesses**, not eternal handwritten Rust
bootstrap. Use `@main` / `@pvd` / `@dep` (from `di.layer`) to compose a runnable
program.

## Minimal path (recommended demo)

```bash
# One-shot: generate + run real CreateItem handler (RT-003)
scripts/run_local_example.sh
```

Or manually:

```bash
veil gen examples/local_run.veil -o /tmp/local_run -t rust
cd /tmp/local_run && cargo run -p veil_bin
```

`@main` packages emit `crates/veil_bin` with a local harness that:

1. Constructs **`InProcessBus`** (from `veil_shared`, RT-001/004)
2. Wires **memory adapters** for ports
3. Calls a generated **application service** (not echo)

Library-only packages (no `@main`) generate context crates without `veil_bin`.

DI-only composition still works via `examples/di_example.veil` (`@pvd` / `@dep`).

3. Check dual-loop quality:

```bash
veil check examples/di_example.veil
```

## What already works

| Capability | Status |
|------------|--------|
| `@main` composition into generated main | Working via `di.layer` + rust codegen |
| `@dep` / `@pvd` DI graph | Working (INV-001: role:dependency) |
| Multi-context Bus orchestration | Opt-in when steps have `ctx` refs **and** layer defines routing traits (INV-003) |
| Smart constructors / timestamps | `rust.layer` `constructor_policy` (INV-002) |

## Layout (RT-021)

Generated workspaces with `@main` emit:

```text
Cargo.toml                 # workspace members include crates/veil_bin
crates/veil_shared/
crates/<context>/
crates/veil_bin/
  Cargo.toml               # [[bin]] name = "veil_bin"
  src/main.rs              # harness main
```

Verify: `cargo metadata --no-deps | jq '.packages[] | select(.name=="veil_bin")'`.

## VEIL packages for Bus / HTTP (RT-022)

- `layers/harness.layer` — prompts + patterns for `@main` composition
- `InProcessBus` generated into `veil_shared` when Bus is declared
- Minimal HTTP remains app-specific (axum in app or host); no eternal bootstrap

## `provided_by: "runtime"` (RT-023)

Local harness (`veil_bin`) supplies Bus (and allow-all Auth when declared)
without a handwritten host. Manifest still lists `provided_by: runtime` for
platform hosts; local gen emits concrete impls so `cargo run -p veil_bin` works.

## HTTP local harness (`@main` without `link veil_server`)

When a product package has `@main` and does **not** link `veil_server`, codegen
emits `crates/veil_bin` as a small **axum REST harness** (RT-001 / RT-003):

| Service name pattern | Method | Path |
|---------------------|--------|------|
| `ListThings` | GET | `/api/things?…` |
| `GetThing` | GET | `/api/things/{id}` |
| `CreateThing` | POST | `/api/things` |
| `UpdateThing` | PUT | `/api/things/{id}` |
| `DeleteThing` | DELETE | `/api/things/{id}` |

- **List\*** inputs are taken from **query string** (e.g. `?tenant_id=`), not random UUIDs.
- **Create/Update** inputs from JSON body.
- Handlers call generated application `fn`s with wired `Deps` (port adapters).

This is the intentional **local product API** surface for dual-loop / Vite proxy —
not the Bus message protocol.

Frontends should call **relative** `/api/...` and proxy to `dev_port` (see
`veil.toml` + Vite `server.proxy`). Use normal `fetch` / WebSocket clients in UI
stores — do **not** route browser traffic through the Bus.

**Author `@proxy` on the UI package** so typescript/sveltekit5 gen emits Vite
proxy config (do not hand-edit forever under `generated/`):

```veil
@proxy("/api", "http://127.0.0.1:3000")   # BEFORE `app` — leading annotation
app MyUi
  …
```

Layer: `layers/sveltekit5.layer` (`match * where has_annotation("proxy")`).
IDE dual-loop ▶ toolbar: `veil.toml` `[[targets]]` + `/api/dev/*` — see
[IDE_AGENT_PLATFORM.md](./IDE_AGENT_PLATFORM.md).

### Bus (backend) vs REST/WebSocket (frontend)

| Surface | Who | Role |
|---------|-----|------|
| **Bus** | Backend services / contexts | Inter-process & multi-context messaging (commands, events, sagas). Not a browser transport. |
| **HTTP REST harness** | `@main` product bin | Local dual-loop + product API (`/api/...`). What Vite proxies to. |
| **WebSockets / auth** | App / host layer | Session auth, cookies, JWT, WS upgrades — sit on the HTTP host, not on Bus envelopes. |

Elevating REST/auth into the Bus would force a paradigm on frontend developers who
already code `fetch` into stores. Keep Bus for **server-side** IPC; keep HTTPS +
auth middleware on the **product host** edge.

### Cloud SDK adapters (via `.stub` only)

`.stub` files declare third-party crate APIs. The engine has **no SDK-specific
knowledge** (no hard-coded Dynamo/S3/aws-config). Stubs may also declare:

| Stub directive | Purpose |
|----------------|---------|
| `cargo_features a, b` | Cargo features for the crate |
| `cargo_deps name=ver` | Companion crates (e.g. `aws-config=1`) |
| `types_module types` | Model types live under `crate::types::…` |
| `root_types Client, Config` | Types that stay at crate root |
| `harness_field Client """…"""` | Rust expr for `@field(client: Client)` in the local harness |

Adapter `impl` bodies that call stub types are lowered generically (fluent
chains, PascalCase enum variants, fallible `.send()` → `.await.map_err(...)?`).

```bash
# Example dual-loop (env vars are whatever your stub's harness_field expects)
export AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_REGION=us-east-1
export DYNAMO_ENDPOINT=http://127.0.0.1:4566 DYNAMO_TABLE=wear_test_initiatives
VEIL_LAYERS_DIR=…/veil/layers veil gen wear_test.veil -t rust -o generated/backend
cd generated/backend && cargo run -p veil_bin
```

Empty bodies still emit `Err(External("not configured"))` for pure-runtime placeholders.
Do not invent `self.client` without `@field` + a stub `harness_field` (or `Default`).

## Product host (CAP-002 / CAP-006)

```bash
veil gen runtime/src/host.veil -t rust -o runtime/generated-host
cd runtime/generated-host && cargo run -p veil_bin
```

`link veil_server` + `@main` emits `ProductHost::listen` (IDE multi + SPA +
`PATCH /api/config`). Live Bus dispatch for storage still mounts from the
`runtime/bootstrap` trampoline until generated handlers fully replace it.

## Known gaps

| Gap | Story |
|-----|--------|
| Empty adapter bodies still emit `todo!` | GEN-001/002 (flagged by escape diagnostics) |
| Full axum host package in pure VEIL | CAP-002 ProductHost (done); bus body DI ongoing |

## Do not reintroduce

Permanent handwritten app harnesses in `runtime/bootstrap` as the only path.
Stage-0 may remain a thin `cargo` entry that calls generated `@main` only.

## Host modes (RT-002)

| Mode | Who wires deps | When |
|------|----------------|------|
| **App harness** | VEIL `@main` / `@pvd` constructs adapters | Local run, custom deploy |
| **Host harness** | External host reads `manifest.json`, injects `provided_by: "runtime"` | Shared platform |

`provided_by: "runtime"` in generated manifests means the host must supply
that trait (e.g. Bus). App mode needs no host if all deps are constructible.

## Related

- Product model: `stories/README.md`
- Runtime harness backlog: `stories/70-runtime-harness.md`
- Server API: `docs/SERVER.md`
- Agents (Rig): `docs/AGENT.md`
- ACP research: `docs/ACP_SPIKE.md`
