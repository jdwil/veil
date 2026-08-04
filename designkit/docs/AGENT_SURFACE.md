# designkit — Agent Surface Contract

Machine-readable UI contracts so agents can understand **stock** components
(how the control works / is configured) and **product** binding (which entity,
which actions, which routes).

## Attributes (DOM)

| Attribute | Purpose |
|-----------|---------|
| `data-veil-role` | Stable stock role id (`collection`, `create-form`, `context-menu`, …) |
| `data-veil-agent` | JSON `AgentSurface` (stock + product + runtime) |

Do **not** overload ARIA for agent metadata.

## Shape (`AgentSurface`)

```ts
{
  version: 1,
  role: string,              // stock role
  stock: {
    component: string,       // Svelte/VEIL comp name
    purpose: string,         // one-liner
    howTo: string[],         // interaction steps for an agent
    config: Record<string, { type: string; desc: string; default?: unknown }>
  },
  product?: {
    intent: string,          // e.g. list-wear-tests
    entity?: string,         // e.g. WearTest
    entityLabel?: string,    // e.g. Wear Test
    actions?: AgentAction[],
    api?: Record<string, string>,
    fields?: { id: string; label: string; required?: boolean; type?: string }[],
    notes?: string[]
  },
  runtime?: Record<string, unknown>  // live: loading, itemCount, open, …
}

type AgentAction = {
  id: string
  label: string
  href?: string
  hrefTemplate?: string   // /wear-tests/{id}
  via?: string            // context-menu | primary | empty-state | form-submit
  confirm?: boolean
  method?: string         // navigate | click | api
}
```

## Layers

1. **Stock** (designkit) — always present; describes the reusable control.
2. **Product** — passed via `agent={{ intent, entity, actions, … }}` from the page.
3. **Runtime** — derived from live props (loading, counts, dialog open).

An agent that wants to “add an item” on a listing page:

1. Finds `[data-veil-role=collection]`
2. Reads `stock.howTo` → primary action / empty action
3. Reads `product.entity` + `product.actions[id=create]` → knows it is adding a **Wear Test**
4. Navigates `href` or follows the form surface on the create route

## Collector

Mount once per app (layout or page):

```svelte
<AgentSurface />
```

Exposes `window.__veilAgentSurface` = `{ collectedAt, surfaces: AgentSurface[] }`
and a custom event `veil:agent-surface`.

## Offline catalog

`docs/agent-catalog.json` mirrors stock contracts for prompt compilation without a browser.

## Authoring rules (future components)

1. Edit **`main.veil`** only (VEIL source of truth — not hand-written Svelte trees).
2. Pick a stable `role` (kebab-case).
3. Document `purpose`, `howTo`, and every public prop under `config` (STOCK_AGENT in the comp script block).
4. Accept optional `agent` product prop; merge into `data-veil-agent`.
5. Put `data-veil-role` + `data-veil-agent` on the **root interactive element**.
6. Update `docs/agent-catalog.json` and this doc when adding roles.
7. Gen: `veil gen main.veil -t typescript -o generated/library`
