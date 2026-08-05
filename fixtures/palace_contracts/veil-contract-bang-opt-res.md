# veil-contract-bang-opt-res

**Type:** Concept  
**Summary:** Bang on decl = fallible; bang on call unwraps Res only — Opt stays Opt. Force present with `require` / `.unwrap()`.

## Contract

- `name!(…)` on **declaration** → method is fallible (`Res!` / `Res!<T>`).
- `Opt<T>` = maybe absent; `Res!` / `Res!<T>` = fallible.
- **Call site (ACS-010 portable, current engine):** `x = repo.find!(id)` yields **`Opt<T>`**
  when declared `-> Opt<T>` (try / Res only; no silent NotFound).
- **Force present:** `require repo.find!(id)` → `T`, or interim `.unwrap()` on `Opt`.
- **Allowed after bang when still Opt:** `.is_some()`, `.is_none()`, match / if-let.
- **Forbidden:** assuming `find!` already yields bare `T`, or inventing `find!!`.
- Opt/Res are portable type formers. Dual-loop NotFound is force-present policy,
  not part of the `!` glyph.

## Example

```
port ItemRepo
  find!(id: Id) -> Opt<Item>
  save!(item: Item)

svc GetItem
  input
    id: Id
    @dep item_repo: ItemRepo
  step load
    item = item_repo.find!(id)   # item: Opt<Item>
    ret item

svc UpdateItem
  input
    id: Id
    @dep item_repo: ItemRepo
  step load
    item = require item_repo.find!(id)   # item: Item
    item_repo.save!(item)
    ret item
```

**Source of truth:** `docs/BANG_CONTRACT.md` · SL-001 in `docs/SEMANTIC_LOWERING.md`
