# veil-contract-stubs

**Type:** Concept  
**Summary:** `.stub` declares third-party crate API; engine does not hardcode SDKs. Use `@field` + `harness_field`. Versioned; platform catalog + project pin; never hand-write full SDKs.

## Contract

- Colocate `stubs/name.stub` or adjacent `name.stub`; package `use name`.
- **Version required:** `stub name <semver>` (+ provenance: `@generated`, `surface`, fingerprint).
- **Resolve order:** project → platform (`VEIL_STUBS_DIR` / `runtime/src/stubs` / S3 `stubs/platform/…` via DDB `STUB#/META`).
- Stub policy: `cargo_deps`, `harness_field Type """…"""`, optional `types_module` / `root_types`.
- Adapter: `@field(sns: aws_sdk_sns.Client)` + `@env(VAR)` — crate-qualify stub types; name the field after the crate (`sns` / `http`), never bare `Client` when several stubs export it. Harness wires from stub recipe or `Default`.
- `@env(TABLE_NAME)` → `self.table_name` (full lowercased var). `DATABASE_URL` → `self.pool`.
- Stub method signatures are the contract. Incremental setters take `(key, StubValue)` (`AttributeValue.S(s)`, `MessageAttributeValue.builder()…`). Never pass `Map<Str, Str>` where the stub names a typed value. `Blob.new(body)` for binary payloads.
- Do not invent `self.client` without `@field` + recipe.
- Do not redefine layer-declared types (`EntityRepo`, `AuthService`, `SagaStep`).
- A message bus is user-land: define a product `port` + stub adapters. `dispatch`/`invoke`/`request` keywords belong in a product layer if wanted.
- **Agents:** NEVER invent full stubs. Use `stub_list` / `stub_get` / `stub_install` / `stub_gen` (or `POST …/stubs/generate`).
- Ladder L3: `fixtures/ladder/l3/`.

## Example

```
# stubs/reqwest.stub (prefer stub_gen / platform install)
stub reqwest 0.13.4
  # @generated veil-stub-gen 1
  # surface full
harness_field Client """
{ reqwest::Client::new() }
"""
  struct Client
    fn new() -> Client

# app.veil
impl HttpPinger for Pinger
  @dep
  @field(http: reqwest.Client)
  @env(API_BASE)
  impl ping(url)
    ret Ok
```

**Source of truth:** `docs/LANGUAGE.md` §stub, `docs/HARNESS.md`, `scripts/seed-stubs-platform.sh`
