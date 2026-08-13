# L1 — CRUD + bang find/list/save

**Skills:** `Opt` / `Res!` portable bang, `guard`, force-present when need `T`.

## DO

- Port methods: `find!` → `Opt<T>`, `list_all!` → `List<T>`, `save!`
- Call site: `x = repo.find!(id)` binds **`Opt<T>`** (bang unwraps Res only)
- Soft GET: `ret` the `Opt` (harness / handler policy may map None → 404)
- Hard path: `require repo.find!(id)` or `.unwrap()` when you need bare `T`
- `guard expr, "msg"` for validation
- Declared `endpoint` on every public HTTP surface

## DON'T

- Assume `find!` already yields bare `T` (ACS-001 transitional is obsolete)
- Invent paths without `endpoint` / `list_routes`
- Skip memory adapter for ports the harness wires

## Contract

See [docs/BANG_CONTRACT.md](../../../docs/BANG_CONTRACT.md).

## Verify

```bash
veil check fixtures/ladder/l1/crud.veil
veil gen fixtures/ladder/l1/crud.veil -o /tmp/ladder-l1 -t rust
cd /tmp/ladder-l1 && cargo check -p veil_bin
```

Or: `make fixture-ladder-l1`
