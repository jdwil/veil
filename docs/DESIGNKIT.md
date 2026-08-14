# designkit — public dual-loop UI design system

| Piece | Path |
|-------|------|
| **Layer** (tokens, motion, constructs, CSS scaffold) | [`layers/designkit.layer`](../layers/designkit.layer) |
| **Stock package** (component bodies) | [`designkit/main.veil`](../designkit/main.veil) |
| **Package docs** | [`designkit/README.md`](../designkit/README.md) |
| **Agent surface** | [`designkit/docs/AGENT_SURFACE.md`](../designkit/docs/AGENT_SURFACE.md) |

**Status:** Ships with VEIL. Product-neutral (no private brand assets).

## What it is

A **public** design system for VEIL dual-loop frontends:

| Piece | Role |
|-------|------|
| **Tokens** | `--dk-*` palette, motion, elevation, glass; semantic aliases `--bg` / `--surface` / `--accent` |
| **Chrome CSS** | Buttons, cards, page headers, tiles, tables, form shells, modals, empty/loading |
| **Motion** | `dk-fade-in`, `dk-fade-up`, tile stagger (`--dk-i`), modal/menu enter; `prefers-reduced-motion` |
| **Constructs** | IDE palette names for CollectionView, CreateFormShell, FormField, dialogs, … |
| **Stock comps** | Implemented `comp`s in `designkit/main.veil` — gen to Svelte |

## Use in a product UI

```veil
pkg MyProductUI
  use designkit          # layer: tokens + constructs (+ sveltekit5)

  app MyApp
    @proxy("/api", "http://127.0.0.1:3000")
    # Reference stock comps by name once the package is a dependency
    …
```

```toml
# consumer veil.toml
[dependencies]
designkit = { path = "../veil/designkit" }
# or on a projects hub:
# designkit = { project = "designkit" }
```

Gen emits designkit’s `src/app.css` / `src/app.html` when the layer is used
(overrides bare sveltekit5 defaults).

### Specialize stock shells

```veil
pkg MyProductUI
  use designkit
  adapt designkit
  # rfn / rpl stock comps when chrome structure must change
```

See [`docs/ADAPT.md`](./ADAPT.md). Prefer **slots** (`children`, `header_actions`, …)
before adapting.

### Rebrand without forking

```css
:root {
  --dk-brand: #4c6ef5;
  --dk-brand-light: #748ffc;
  --dk-brand-dark: #3b5bdb;
  --dk-brand-glow: rgba(76, 110, 245, 0.35);
  --accent: var(--dk-brand-light);
  --accent-hover: #91a7ff;
}
```

Do **not** put private brand colors into `layers/designkit.layer` or `designkit/main.veil`.

## Class quick reference

```text
.btn-primary | .btn-outline | .btn-ghost | .btn-danger
.card | .badge-success | .badge-warning | .badge-info | .badge-error
.dk-page-header | .dk-page-header__title | .dk-page-header__desc
.dk-collection | .dk-tile-grid | .dk-tile | .dk-tile--clickable
.dk-table | .dk-empty | .dk-loading | .dk-spinner
.dk-page-shell | .dk-create-shell | .dk-form-section | .dk-field
```

Stagger lists: set `--dk-i: {index}` on each child.

## Shell apps (sidebar layouts)

Designkit’s global `main { max-width: 1120px; margin: 0 auto; … }` suits
content pages. For app shells that fill a flex pane, give main a local class
and reset (see `ui/src/lib/components/`):

```css
main.shell-main {
  max-width: none;
  margin: 0;
  animation: none;
}
```

## Stock component catalog

| Component | Role |
|-----------|------|
| CollectionView | List / tile grid with view toggle |
| CreateFormShell / DetailShell / DetailField | CRUD page chrome |
| FormField / FormSection / FormProgress | Form building blocks |
| PageHeader / EmptyState / ViewModeToggle | Page chrome |
| StatusPill / EntityIdentity | Cells / identity |
| Modal / ConfirmDialog / AlertDialog / PromptDialog | Overlays |
| ContextMenu | ⋮ actions |
| WizardShell / ChoiceCards / RepeatEditor | Multi-step / repeatable |
| AgentSurface | Publishes `window.__veilAgentSurface` |

## Relationship to private products

| Concern | Public designkit | Private product |
|---------|------------------|-----------------|
| Tokens & motion | Yes | Override `--dk-brand*` |
| Stock comps | Yes | `adapt` when needed |
| Logo / marketing | No | Product assets only |

## Check / gen

```bash
veil check layers/designkit.layer
veil check designkit/main.veil
veil gen designkit/main.veil -t typescript -o designkit/generated/library
```
