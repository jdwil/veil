# veil-contract-inv001-harness

**Type:** Concept  
**Summary:** Engine and dual-loop harness laws — roles not annotation spellings, secrets store+redact, NotFound guards, thin handlers, DELETE query, default-deny auth, CORS order.  
**Links:** veil-harness-devloop, veil-di-layer, veil-codegen-targets-vs-layers, veil-stubs-and-sdks, veil-project-index, veil-contract-veil-authoring

## INV-001 — roles and policies

Engine must not hard-code product annotation names or DDD vocabulary.

- Match **roles** (`role:dependency`, `role:secret`, `role:http_route`, …) from layers.
- Match **policies** and optional `veil.toml` `[codegen]` overrides.
- Packs: `rest_english`, `rest_rpc`, `bus_handle`, `auth_local`.
- Docs: `docs/POLICY_ROLES.md`.

**Don't:** add product field names or annotation strings to `rust.rs` / `expr.rs`.

## Secrets — store vs API JSON

- Domain `@secret` fields **Serialize fully** for repo payloads.
- **Never** only `skip_serializing` on domain types (credentials never stored).
- HTTP: `veil_json_public` strips secret keys + redacts `headers[].value`.

## Errors and guards

- Status from **DomainError variants** (404/400/502), not Display substrings.
- Upstream / `ret Err(...)` → **External** (502).
- Guards: not found / cross-tenant / access denied → **NotFound**.

## ApplicationService vs DomainService

`handler` matching `svc` after bus strip → thin `domain_fn(...).await` only.

## Harness HTTP edge

- DELETE extras → **query**, not body.
- CORS outside API-key; OPTIONS skips key.
- Auth **default-deny** unless `VEIL_DEV=1` or `VEIL_API_KEY` matches.
- Local smoke: `export VEIL_DEV=1`.

## Stubs

- Re-run `veil stub-gen`; no hand-edit of harness recipes.
- AWS: `load_defaults(BehaviorVersion::latest())`.
- reqwest: connect 5s + timeout 30s on Client builder.
