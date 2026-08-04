# designkit (stock package)

Public **stock UI components** for VEIL dual-loop frontends.  
Tokens, motion, and IDE vocabulary live in the system layer:

```text
layers/designkit.layer     # use designkit  → CSS + constructs
designkit/main.veil        # this package  → stock comps
```

## Layout

```text
designkit/
  veil.toml
  main.veil                 # ← EDIT STOCK COMPONENTS HERE
  docs/
    AGENT_SURFACE.md        # agent contract schema
    agent-catalog.json      # offline stock catalog for prompts
  generated/library/        # veil gen output only — do not hand-edit
```

| Artifact | Role |
|----------|------|
| **`main.veil`** | Implementation: stock `comp`s. **Author here.** |
| **`layers/designkit.layer`** | Language + design tokens + app CSS + agent authoring prompt |
| **`veil gen`** | Emits Svelte/TS into `generated/library` |

There is **no** parallel `src/components/` Svelte tree and **no** reverse Svelte→VEIL build.
Templates may contain Svelte markup inside `template """…"""`; that is still owned by this package.

## Consumer products

```veil
pkg WearTestUI
  use designkit
  # adapt designkit   # when specializing stock shells
  …
```

```toml
# consumer veil.toml
[dependencies]
designkit = { path = "../veil/designkit" }
# hub-style:
# designkit = { project = "designkit" }
```

Resolution: dependency roots feed package + layer search so `use designkit` and
`adapt designkit` resolve without ambient sibling discovery.

## Stock components

### CRUD page shells (generic; product fills slots)

| Shell | Role | Primary slots / extension points |
|-------|------|----------------------------------|
| **CollectionView** | List / grid | `tile`, `row`, `header_actions`; `item_href_template` for row/card nav |
| **CreateFormShell** | Create **or** edit form | `children` (fields), `header_actions`, `footer` (danger zone); `mode`, `loading` |
| **DetailShell** | Read-only detail | `header_actions`, `summary`, `children`, `sidebar`, `footer`; `loading` |
| **DetailField** | Read-only field | `value` / `pre` / `mono`, or `children` for custom (e.g. StatusPill) |
| **WizardShell** | Multi-step wizard | step chrome + progress |
| **ChoiceCards** | Single-select card group | choice tiles |
| **RepeatEditor** | Repeatable field groups | add/remove rows |

### Atoms & chrome

| Component | Role |
|-----------|------|
| **FormField** / **FormSection** / **FormProgress** | Form building blocks |
| **ContextMenu** | ⋮ row/tile actions (portaled) |
| **ConfirmDialog** / **AlertDialog** / **PromptDialog** | Replacements for native dialogs |
| **Modal** | Shared overlay (`<dialog>` top layer) |
| **PageHeader**, **EmptyState**, **ViewModeToggle** | Page chrome |
| **StatusPill**, **EntityIdentity** | Cells / identity |
| **AgentSurface** | Collects `data-veil-agent` → `window.__veilAgentSurface` |

Product owns domain (entity fields, API, routes). Kit owns chrome + agent roles.
Further family-wide structural changes: VEIL **`adapt`** on designkit stock.

## Check / gen

```bash
# From VEIL repo root
veil check designkit/main.veil
veil gen designkit/main.veil -t typescript -o designkit/generated/library
```

After editing comps or `layers/designkit.layer` CSS, regenerate consumers so
adapted designkit comps pick up changes.

## Branding

Default accent is neutral teal (`--dk-brand*`). Products rebrand by overriding
tokens on `:root` — do not fork this package for private brand colors.  
See [`docs/DESIGNKIT.md`](../docs/DESIGNKIT.md).
