# veil-contract-routes

**Type:** Concept  
**Summary:** `@route` is authoritative. Name-derived List/Get/Create is fallback only. Use `list_routes`.

## Contract

- Public handlers: `@route("METHOD /path")` (e.g. `GET /api/items`).
- Never invent paths in English — call `list_routes` or `read_generated(what=routes)`.
- Name-derived routes exist only when `@route` is missing (fallback).
- Prefer stock examples + ladder fixtures that already declare routes.

## Example

```
@route("GET /api/items")
svc ListItems
  input
    @dep item_repo: ItemRepo
  step query
    items = item_repo.list_all!()
    ret items
```

Agent: after edit → `list_routes` → `http_request(path="/api/items", target=backend)`.

**Source of truth:** `docs/HARNESS.md` (ACS-005)
