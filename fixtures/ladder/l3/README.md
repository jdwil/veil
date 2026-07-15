# L3 — Stub SDK + adapter

**Skills:** `.stub`, `harness_field`, `@field`, `@env`, `cargo_deps`.

## DO

- Colocate `reqwest.stub` (or any crate stub) next to the package
- `use reqwest` (stub name) in the package
- Adapter: `@field(client: Client)` + `@env(API_BASE)` (or similar)
- Put construction recipe on the stub: `harness_field Client """…"""`
- Keep a memory path for domain ports so HTTP list still works offline

## DON'T

- Invent `self.client` without `@field` + stub `harness_field` (or `Default`)
- Hardcode SDK crates in the engine (stubs own policy)
- Call network APIs in CI green path unless intentionally integration-tested

## Verify

```bash
veil check fixtures/ladder/l3/app.veil
veil gen fixtures/ladder/l3/app.veil -o /tmp/ladder-l3 -t rust
cd /tmp/ladder-l3 && cargo check -p veil_bin
```

Or: `make fixture-ladder-l3`

Harness wires `reqwest::Client::new()` from the stub; set `API_BASE` at runtime if you extend `ping` to use `self.base`.
