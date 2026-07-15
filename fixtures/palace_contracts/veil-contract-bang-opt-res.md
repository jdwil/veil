# veil-contract-bang-opt-res

**Type:** Concept  
**Summary:** Bang on decl = fallible; bang on call unwraps Res/Opt to T. Never `.unwrap()` after `find!`.

## Contract

- `name!(…)` on **declaration** → method is fallible (`Res!` / `Res!<T>`).
- `Opt<T>` = maybe absent; `Res!` / `Res!<T>` = fallible.
- **Call site (current engine):** `x = repo.find!(id)` yields **T** (try + NotFound for Opt).
- **Forbidden after bang:** `.unwrap()`, `.is_some()`, `.is_none()` on the bound result.
- Opt/Res are portable; Opt→NotFound on bang is product dual-loop policy (see ACS-010).

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
    item = item_repo.find!(id)   # item: Item — no unwrap
    ret item
```

**Source of truth:** `docs/BANG_CONTRACT.md`
