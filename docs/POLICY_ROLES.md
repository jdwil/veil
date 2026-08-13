# Policy roles & layer policies (INV-001)

The engine **must not** encode product vocabulary or annotation spellings
(`"route"`, `"dep"`, `"Handle…"`, `"AuthService"`, …). Layers declare **roles**
and **policies**; codegen keys off those.

## Annotation roles

On any construct annotation in a `.layer` file:

```text
ann
  dep: "Injected dependency" field role:dependency
  # API @route / role:http_route was removed from ddd (flip).
  # HTTP surface is construct role http_endpoint (harness.layer).
```

| Role | Purpose | Declared in |
|------|---------|-------------|
| `dependency` | DI field / input | `di.layer` (`@dep`) |
| `provider` | Factory / provider fn | `di.layer` (`@pvd`) |
| `main` | Composition-root contribution | `di.layer` (`@main`) |
| `secret` | Omit from outbound serialization | `di.layer` (`@secret`) |
| `shared` | Shared ownership (e.g. Arc) | `di.layer` (`@shared`) |
| `http_endpoint` | Dual-loop REST surface | `harness.layer` (`endpoint`) |
| `ui_route` | Svelte page/layout URL path | `svelte5.layer` (`@route`) |
| `permission` | Required permission claim | `ddd.layer` (`@auth`) |
| `invariant` | Smart-constructor validation | `ddd.layer` (`@invariant`) |
| `adapter_env` | Required env vars for adapters | `ddd.layer` (`@env`) |
| `adapter_field` | Stub harness field wiring | `ddd.layer` (`@field`) |
| `runtime_strategy` | Runtime provider key | `ddd.layer` (`@strategy`) |

Engine API (examples): `registry.is_dependency_annotation(name)`,
`registry.construct_has_role(c, "http_endpoint")`, `registry.field_is_secret(field)`.

Products may **rename** annotations in a custom layer as long as the **role**
stays the same. Engine code never matches the surface name.

## Layer policy blocks

Top-level blocks in a `.layer` (merged across `use`d layers):

### `bus_policy`

```text
bus_policy
  strip_name_prefix Handle
```

Bus message keys strip an optional prefix (e.g. `HandleCreateX` → `CreateX`).
**No `Handle` string in the engine.**

### `auth_policy`

```text
auth_policy
  service_trait AuthService
```

Which trait name gets the local allow-all harness impl. Empty = no special auth.

### `http_name_policy`

```text
http_name_policy
  list_prefix List
  get_prefix Get
  create_prefix Create
  update_prefix Update
  delete_prefix Delete
  path_prefix /api/
```

Name-derived REST when no `role:http_route` annotation is present.
`ListInitiatives` → `GET /api/initiatives`. Override in product layers or
`rust.layer`.

### `harness_policy`

Reusable local-harness knobs (listen, CORS, auth, when to emit `veil_bin`).
**Same tokens** as `veil.toml` `[harness]`. Codegen does **not** emit from
this yet — parse/merge only. See `docs/DESIGN_CONFIGURABLE_HARNESS.md`.

```text
harness_policy
  profile axum_http          # axum_http | axum_rpc | product_host
  bin veil_bin
  listen_env PORT
  listen_default 3000
  health /health             # or none
  cors localhost             # localhost | env | permissive | none
  cors_outside_auth true     # orthogonal to cors origin mode
  auth api_key               # none | api_key
  emit_bin on_entry          # on_entry | never
  bus_wire explicit          # explicit | synthesize_runtime
  collide error              # error | prefix_crate
  bind_defaults method       # method | none
  delete_extras query        # query | body | error
  provided_runtime_trait Bus # layer-owned names; engine does not hard-code "Bus"
```

| Knob | Layer token | Toml key | Default (`axum_http`) |
|------|-------------|----------|------------------------|
| profile | `profile axum_http` | `profile` | `axum_http` |
| bin | `bin veil_bin` | `bin` | `veil_bin` |
| listen env | `listen_env PORT` | *(part of `listen`)* | `PORT` |
| listen default | `listen_default 3000` | `listen = "0.0.0.0:3000"` | `0.0.0.0:3000` |
| health | `health /health` | `health` | `/health` (`none` clears) |
| CORS origins | `cors localhost` | `cors` | `localhost` (not `permissive`) |
| CORS vs auth | `cors_outside_auth true` | `cors_outside_auth` | `true` |
| auth | `auth api_key` | `auth` | `api_key` |
| emit bin | `emit_bin on_entry` | `emit_bin` | `on_entry` |
| collide | `collide error` | `collide` | `error` |
| compat | *(none)* | `compat` | `off` for new `veil init`; existing toml without `[harness]` still `auto` |
| bind defaults | `bind_defaults method` | `bind_defaults` | `method` |
| delete extras | `delete_extras query` | `delete_extras` | `query` |

Merge: **documented defaults → layers (`use` order) → `veil.toml` `[harness]`**.
`[harness.wire]` is a field→adapter map (compose overrides; unused by codegen yet).

**Construct roles** (on `construct` bodies, not annotations):

```text
construct HttpEndpoint
  kw endpoint
  mt struct
  role http_endpoint
  has
    method: ident
    path: path
    handle: ident
    bind: struct
```

Engine matches `role http_endpoint` / `deps_bundle` / `compose` / `runtime_provider`.
It does **not** match keywords (`endpoint`, `ctx`) or DDD names. `has` field
names become `ConstructSpec.config_keys` (protocol tokens, not domain types).

### `identity_policy` / `constructor_policy`

Existing INV-006 / INV-002 blocks — FK suffix / smart-constructor defaults.
See `docs/PRESENTATION.md` and `layers/rust.layer`.

### `declare` / `prompt` / `codegen`

- `declare` — raw VEIL injected into every package using the layer (Bus, saga
  coordinator, AuthService trait surface, …).
- `prompt` — LLM guidance only (ignored by codegen).
- `codegen <target>` — emission templates.

**Section transitions:** entering `declare` / `prompt` / `codegen` clears the
others. A long `prompt` followed by comments then `declare` must not swallow
declarations (regression: `prompt_then_declare_preserves_declarations`).

## Template conditions (codegen blocks)

Prefer **roles** over annotation spellings:

```text
match struct where has_role("dependency")
match fn where has_role("main")
```

Still supported for layer self-reference: `has_annotation("dep")`.

Placeholders:

- `{{route}}` — `role:ui_route` (svelte page/layout) or leftover `role:http_route`
- `{{annotation_value:name}}` / `{{annotation_arg:name:N}}` — generic, any name

## Catalog of shipped layers (policy surface)

| Layer | Policies / roles |
|-------|------------------|
| `di.layer` | dependency, provider, main, secret, shared |
| `ddd.layer` | `use rest_english` + `use bus_handle` + **`use harness`**; auth/identity; invariant, adapter_*, strategy; declare Bus/Auth/saga |
| `svelte5.layer` | `role:ui_route` on page/layout `@route` (not `http_route`) |
| `harness.layer` | `endpoint` / `deps` / `compose`; harness_policy |
| `rest_english.layer` | http_name_policy migrate helper (codegen unused when compat=off) |
| `rest_rpc.layer` | clears name-derived REST; require declared `endpoint` |
| `bus_handle.layer` | bus_policy strip `Handle` |
| `auth_local.layer` | auth_policy.service_trait AuthService (AllowAllAuth) |
| `rust.layer` | constructor_policy; `use rest_english` |
| `harness.layer` | docs for dual-loop roles + bus_policy; `harness_policy` (parse-ready, not shipped on this layer yet) |

## What still lives in the engine (acceptable)

- **HTTP verbs** as protocol (`GET`/`POST`/…) when parsing a route string
- **Rust/TS target mechanics** (async_trait, axum, serde)
- **Generic shapes** (`List`/`Map`/`Opt`/`Res`) — language, not domain
- **Residual:** `InProcessBus` method bodies still name dispatch/invoke/request
  matching the declared `Bus` trait surface from the layer (long-term: emit
  from trait methods only)

## Product overrides (implemented)

### A. `veil.toml` `[codegen]` overrides

```toml
[codegen]
# Applied after layers. Absent keys leave layer policy alone.
bus_strip_prefix = "Handle"          # "" or "none" clears
auth_service_trait = "AuthService"
http_path_prefix = "/api/v1/"
http_list_prefix = "List"
http_get_prefix = "Get"
http_create_prefix = "Create"
http_update_prefix = "Update"
http_delete_prefix = "Delete"
```

Merge order: **builtin defaults → layers (load order) → veil.toml**.

Wired in `LayerRegistry::for_veil_file` via `apply_codegen_overrides`.

### `veil.toml` `[harness]` overrides

```toml
[harness]
profile = "axum_http"
compat = "off"
cors = "localhost"
auth = "api_key"
emit_bin = "on_entry"
health = "/health"          # "none" disables

[harness.wire]
item_repo = "PgItemRepo"
```

Same tokens as layer `harness_policy`. Applied after layers via
`apply_harness_overrides`. Codegen emits `veil_bin` from HarnessIR.

### B. Named policy packs (shipped)

| Layer | Effect |
|-------|--------|
| `rest_english` | List/Get/Create/Update/Delete + `/api/` |
| `rest_rpc` | Clears name-derived prefixes (`none`); require declared `endpoint` |
| `bus_handle` | `strip_name_prefix Handle` |
| `auth_local` | `service_trait AuthService` → AllowAllAuth for dual-loop |

`ddd.layer` / `rust.layer` `use rest_english` (+ ddd also `bus_handle`, `auth_local`).
Products can `use rest_rpc` after ddd (later wins) or set `[codegen]` clears.

### C / D (not implemented)

- Annotation aliases (`inject → dep`) — only if a product renames surfaces.
- Per-package `policy` block in `.veil` — use `[codegen]` or a pack layer first.
