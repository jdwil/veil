# L1 — CRUD + bang find/list/save

**Skills:** `Opt` / `Res!` bang, `guard`, no unwrap after `!`.

## DO

- Port methods: `find!` → `Opt<T>`, `list_all!` → `List<T>`, `save!`
- Call site: `x = repo.find!(id)` then use `x` as `T` (engine unwraps)
- `guard expr, "msg"` for validation
- `@route` on every public handler

## DON'T

- `.unwrap()`, `.is_some()`, `.is_none()` after a bang call
- Invent paths without `@route` / `list_routes`
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
