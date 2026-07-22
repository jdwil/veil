# Policy roles & layer policies (INV-001)

The engine **must not** encode product vocabulary or annotation spellings
(`"route"`, `"dep"`, `"Handle…"`, `"AuthService"`, …). Layers declare **roles**
and **policies**; codegen keys off those.

## Annotation roles

On any construct annotation in a `.layer` file:

```text
ann
  dep: "Injected dependency" field role:dependency
  route: "HTTP method and path" method_path role:http_route
```

| Role | Purpose | Declared in |
|------|---------|-------------|
| `dependency` | DI field / input | `di.layer` (`@dep`) |
| `provider` | Factory / provider fn | `di.layer` (`@pvd`) |
| `main` | Composition-root contribution | `di.layer` (`@main`) |
| `secret` | Omit from outbound serialization | `di.layer` (`@secret`) |
| `shared` | Shared ownership (e.g. Arc) | `di.layer` (`@shared`) |
| `http_route` | Dual-loop REST surface | `ddd.layer` (`@route`) |
| `invariant` | Smart-constructor validation | `ddd.layer` (`@invariant`) |
| `adapter_env` | Required env vars for adapters | `ddd.layer` (`@env`) |
| `adapter_field` | Stub harness field wiring | `ddd.layer` (`@field`) |
| `runtime_strategy` | Runtime provider key | `ddd.layer` (`@strategy`) |

Engine API (examples): `registry.is_dependency_annotation(name)`,
`registry.http_route_annotation(construct)`, `registry.field_is_secret(field)`.

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

- `{{route}}` — first arg of any `role:http_route` annotation
- `{{annotation_value:name}}` / `{{annotation_arg:name:N}}` — generic, any name

## Catalog of shipped layers (policy surface)

| Layer | Policies / roles |
|-------|------------------|
| `di.layer` | dependency, provider, main, secret, shared |
| `ddd.layer` | `use rest_english` + `use bus_handle`; auth/identity; http_route, invariant, adapter_*, strategy; declare Bus/Auth/saga |
| `rest_english.layer` | http_name_policy (List/Get/… `/api/`) |
| `rest_rpc.layer` | clears name-derived REST |
| `bus_handle.layer` | bus_policy strip `Handle` |
| `rust.layer` | constructor_policy; `use rest_english` |
| `harness.layer` | docs for dual-loop roles + bus_policy |

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

### B. Named policy packs (shipped)

| Layer | Effect |
|-------|--------|
| `rest_english` | List/Get/Create/Update/Delete + `/api/` |
| `rest_rpc` | Clears name-derived prefixes (`none`); require `role:http_route` |
| `bus_handle` | `strip_name_prefix Handle` |

`ddd.layer` / `rust.layer` `use rest_english` (+ ddd also `use bus_handle`).
Products can `use rest_rpc` after ddd (later wins) or set `[codegen]` clears.

### C / D (not implemented)

- Annotation aliases (`inject → dep`) — only if a product renames surfaces.
- Per-package `policy` block in `.veil` — use `[codegen]` or a pack layer first.
