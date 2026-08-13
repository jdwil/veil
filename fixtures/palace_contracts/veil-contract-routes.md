# veil-contract-routes

**Type:** Concept  
**Summary:** Declared `endpoint` is the HTTP surface. API `@route` on svc/handler is gone. Use `list_routes`.

## Contract

- Public handlers: first-class `endpoint Name METHOD /path -> Handler` (or field form).
- Never invent paths in English — call `list_routes` or `read_generated(what=routes)`.
- Do **not** write `@route` on `svc` / `handler`. That role was removed from `ddd.layer`.
- Svelte page `@route("/path")` stays as `role:ui_route` and must **never** gain `role:http_route`.
- Name-derived List/Get exists only when `[harness] compat = "auto"` (legacy). New `veil init` is `compat = "off"`.
- `use ddd` includes `endpoint` / `deps` / `compose` (`ddd` uses harness). Opt-out: `[harness] emit_bin = "never"`.

## Example

```
endpoint ListItemsHttp GET /api/items -> ListItems
  bind
    tenant_id: tenant

svc ListItems
  input
    @dep item_repo: ItemRepo
    tenant_id: Id
  step query
    items = item_repo.list_all!()
    ret items
```

Agent: after edit → `list_routes` → `http_request(path="/api/items", target=backend)`.

**Source of truth:** `docs/HARNESS.md`, `docs/DESIGN_CONFIGURABLE_HARNESS.md`
