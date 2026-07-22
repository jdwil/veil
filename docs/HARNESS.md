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
| `@main` composition into generated main | Working via `di.layer` + rust codegen (`role:main`) |
| `@dep` / `@pvd` DI graph | Working (INV-001: `role:dependency` / `role:provider`) |
| `@route` dual-loop REST | Working (`role:http_route`; never hard-coded `"route"`) |
| Name-derived REST | `rest_english` pack (`ddd`/`rust` use it); override via `rest_rpc` or `[codegen]` |
| Bus message key strip | `bus_handle` pack; `[codegen] bus_strip_prefix` |
| Product policy knobs | `veil.toml` `[codegen]` (see POLICY_ROLES.md) |
| Auth default | Open only with `VEIL_DEV=1`; else require `VEIL_API_KEY` |
| Secret redaction | HTTP via `veil_json_public` (role:secret + header values); storage full Serialize |
| ApplicationService | Thin delegate to DomainService twin when names match after bus strip |
| DELETE extras | Query string (`?tenant_id=`), not JSON body |
| Multi-context Bus orchestration | Opt-in when steps have `ctx` refs **and** layer defines routing traits (INV-003) |
| Smart constructors / timestamps | `rust.layer` `constructor_policy` (INV-002) |

Full role catalog and policy blocks: [`POLICY_ROLES.md`](./POLICY_ROLES.md).

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

## Multi-package local harness (`[dev].packages`)

Local dual-loop often needs **several product packages in one process** so they
can share an HTTP surface (e.g. `wear_test` + `dlx_core` / IAAA). Production
still deploys packages independently.

### `veil.toml`

```toml
[dev]
# Absolute or project-relative paths to additional .veil packages
packages = ["/path/to/dlx_core/dlx_core.veil"]
# packages = ["../dlx_core/dlx_core.veil"]
```

When `packages` is **non-empty** and the backend target is `rust`, dual-loop:

1. `veil gen <primary> -o generated/backend --no-prune`
2. `veil gen <each dev package> -o … --no-prune` (keeps sibling crates)
3. `veil gen-harness <primary> <dev…> -o …` — one combined `veil_bin`
4. Prune crates not path-dep’d by that harness; merge workspace members/deps

When `packages` is **empty / omitted**, single-package gen only (prunes stale
crates from prior multi gens).

### Manual CLI (same as dual-loop)

```bash
veil gen wear_test.veil -o generated/backend --no-prune
veil gen /path/to/dlx_core/dlx_core.veil -o generated/backend --no-prune
veil gen-harness wear_test.veil /path/to/dlx_core/dlx_core.veil -o generated/backend
cd generated/backend && cargo run -p veil_bin
```

Combined `veil_bin` wires **all** context crates’ adapters and merges REST
routes into one axum server (`/health` + each package’s `/api/…`).

### Flags

| Flag / command | Role |
|----------------|------|
| `[dev].packages` in `veil.toml` | Declares which extra packages join the local harness |
| `veil gen --no-prune` | Keep other packages’ crates while genning the next one |
| `veil gen-harness A.veil B.veil -o dir` | Rewrite `veil_bin` to wire A+B together |

Reload: dual-loop re-reads `veil.toml` on each gen, so toggling `[dev].packages`
does not require a full IDE restart (restart `veil serve` after rebuilding the
binary).

---

## HTTP local harness (modules → `veil_bin`)

Packages with **context modules** get `crates/veil_bin` as a small **axum REST
harness** for local dual-loop (RT-001 / RT-003). **`@main` is optional** for
this path. `@main` is still used for Bus composition demos and ProductHost
(`link veil_server`).

### Route selection (AGT-026 · ACS-005)

1. **Authoritative:** `@route("METHOD /path")` on the svc/handler — prefer this on all public HTTP surface.
2. Path-only `@route("/path")` keeps the name-derived method.
3. **Fallback only** (legacy): name prefixes when no `@route`:

| Service name pattern | Method | Path |
|---------------------|--------|------|
| `ListThings` | GET | `/api/things?…` |
| `GetThing` | GET | `/api/things/{id}` |
| `CreateThing` | POST | `/api/things` |
| `UpdateThing` | PUT | `/api/things/{id}` |
| `DeleteThing` | DELETE | `/api/things/{id}` |

### Agent closed loop (AGT-020–028 · ACS-002)

After editing a package that affects the backend:

1. Host **smoke**: gen + `cargo check` (rejected writes restore previous file).
2. On WRITE REJECTED: `dev_logs` / `smoke_status` **before** large rewrites.
3. `list_routes` / `read_generated(what=harness)` — real paths.
4. `dev_restart` (or auto-restart after smoke — ACS-004) — reload `cargo run`.
5. `http_request(path, target=backend)` — live probe (127.0.0.1 + `dev_port` only).

**Bang / Opt / Res:** [`BANG_CONTRACT.md`](./BANG_CONTRACT.md).  
**Multi-package local harness fixture:** `fixtures/multi_harness/` (ACS-003).  
**Complexity ladder L0–L3:** `fixtures/ladder/` (ACS-006) — `make fixture-ladder`.  
**Smoke scope (ACS-012):** after a package edit, dual-loop checks primary context
crate(s) of the changed file (`cargo check -p …`), not the whole workspace /
`veil_bin`, unless `VEIL_AGENT_SMOKE_FULL=1`. See [AGENT.md](./AGENT.md).

Set `VEIL_AGENT_SMOKE=0` only as an escape hatch.

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
| `row_type_derives Path` | Multi-field domain types get these derives (e.g. `sqlx::FromRow`) |
| `wrapper_type_derives Path` | Single-field wrappers get these derives (e.g. `sqlx::Type`) |
| `wrapper_type_attrs inner` | Extra attrs on wrappers (e.g. `sqlx(transparent)` → `#[sqlx(transparent)]`) |
| `codegen_imports Path` | Extra `use` lines when the stub is active (e.g. `sqlx::PgPool`) |
| `rust_name VeilName RustName` | VEIL type → Rust type when names differ (`Pool` → `PgPool`) |
| (on struct) `typed_variant name` | Free-fn for typed `new` (e.g. `query_as` when return is a domain type) |
| (on struct) `typed_type_params …` | Turbofish template; `return_type` = enclosing domain type (default `_, return_type`) |

**Engine invariant:** rust codegen never hardcodes `sqlx::…` / AWS symbols. Drivers
declare policy on the `.stub`; `rust.rs` / `expr.rs` apply it generically.

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
