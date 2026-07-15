# veil-contract-bang-opt-res

**Type:** Concept  
**Summary:** Bang on decl = fallible; bang on call unwraps Res/Opt to T. Never `.unwrap()` after `find!`.

## Contract

- `name!(…)` on **declaration** → method is fallible (`Res!` / `Res!<T>`).
- `Opt<T>` = maybe absent; `Res!` / `Res!<T>` = fallible.
- **Call site (current engine):** `x = repo.find!(id)` yields **T** (try + NotFound for Opt).
- **Forbidden after bang:** `.unwrap()`, `.is_some()`, `.is_none()` on the bound result.
- Opt/Res are portable. **ACS-010 preferred (not default yet):** bang = Res try only;
  Opt stays Opt; force-present via `require` / layer policy — not silent NotFound on every `!`.
  **Current engine:** still Opt→NotFound on bang (transitional).

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
