# veil-contract-stubs

**Type:** Concept  
**Summary:** `.stub` declares third-party crate API; engine does not hardcode SDKs. Use `@field` + `harness_field`.

## Contract

- Colocate `name.stub`; package `use name`.
- Stub policy: `cargo_deps`, `harness_field Type """…"""`, optional `types_module` / `root_types`.
- Adapter: `@field(client: Client)` + `@env(VAR)` — harness wires from stub recipe or `Default`.
- Do not invent `self.client` without `@field` + recipe.
- Ladder L3: `fixtures/ladder/l3/`.

## Example

```
# reqwest.stub
stub reqwest 0.12
harness_field Client """
{ reqwest::Client::new() }
"""
  struct Client
    fn new() -> Client

# app.veil
impl HttpPinger for Pinger
  @dep
  @field(client: Client)
  @env(API_BASE)
  impl ping(url)
    ret Ok
```

**Source of truth:** `docs/LANGUAGE.md` §stub, `docs/HARNESS.md`
