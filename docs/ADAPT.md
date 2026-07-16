# Package adapt — product specialization (gold standard)

**Status:** **Done** (ADP-000–013) · Stories: [`stories/150-package-adapt.md`](../stories/150-package-adapt.md)  
**Related:** [`LANGUAGE.md`](LANGUAGE.md) (`use` / `link`), package expose, dual-loop IDE

---

## Problem

Stock products (e.g. Wear Test Application) must be **specialized per client**
without:

- Overloading `use` (which means depend on an API boundary — possibly remote Bus)
- OOP “extends” / runtime `super` chains
- Copy-paste forks of entire packages

VEIL is a **transpiler**: specialization should produce **one flattened IR** and
**one** generated body per final symbol (no parent frames on the call stack).

---

## Design decisions (locked)

| Decision | Choice |
|----------|--------|
| Dependency keyword | **`use`** — API / DTOs / layers / stubs; may be remote |
| Specialization keyword | **`adapt`** — pull **base package source** into this compile unit |
| New top-level constructs | Ordinary `agg` / `svc` / `fn` / … (implicit add) |
| Patch existing symbols | **`ins`**, **`rfn`**, **`rpl`**, **`omit`**, **`ren`** |
| Parent behavior | **`stock`** — **transpile-time splice** of ancestor body (not runtime call) |
| Multi-level adapt | Fully **inline / flatten** root → leaf before codegen |
| Platform packages | **`adapt dlx_core` forbidden** (or hard error); platform stays `use` only |
| Diamond adapts | Forbidden unless explicit `order` clause (see ADP-003); default **linear chain** |

---

## Keywords

### `adapt <package>`

```veil
pkg AcmeWearTest
  use ddd
  use application
  use dlx_core
  adapt wear_test
```

- Resolves `wear_test` to **package sources** (not only `expose`).
- May appear multiple times only if **order** is total (prefer **one** product line).
- Chaining: `Acme` adapts `Regional` adapts `wear_test` → merge order is
  root base first, then each adapter outward to the leaf package.

### Patch ops (only on symbols that exist on the adapted base)

| Op | Meaning |
|----|---------|
| **`ins <path>`** | Insert **sub-components** into an existing construct (method, step, field, nested construct). Position optional for steps. |
| **`rfn <path>`** | **Refine** body: new body may contain **`stock`** splices of the prior body. |
| **`rpl <path>`** | **Replace** body entirely; **`stock` illegal**. |
| **`omit <path>`** | Remove symbol (or step) from the product surface. |
| **`ren <path> <new_name>`** | Rename a base symbol; updates internal references in the merged IR. |

### New symbols

```veil
# No keyword — just define
svc AcmeReport
  ...
```

### `stock` (refine only)

| Position | Expansion |
|----------|-----------|
| Statement / step body | Inline prior statements of that symbol |
| Expression (`x = stock`) | Inline as value (last `ret` / expression of prior body) |

After merge, **no `stock` nodes remain** in the IR passed to codegen.

---

## Path addressing

Paths name symbols in the **merged base as seen before this package’s patches**:

```text
CreateInitiative
CreateInitiative.step step1_choose_cohort
Initiative
Initiative.fn mark_vip
Initiative.root                 # aggregate root block (field ins later)
WearTestDashboard
```

**Step position** (for `ins` of steps):

```veil
ins CreateInitiative
  step acme_audit after persist
    ...
  step acme_pre at start
    ...
```

Clauses: `before <step>`, `after <step>`, `at start`, `at end` (default `at end`).

---

## Grammar sketch

```veil
pkg AcmeWearTest
  use ddd
  use dlx_core
  adapt wear_test

  # New top-level — ordinary syntax
  svc AcmeSlackNotify
    input
      text: Str
    step send
      ret Ok

  # Insert method on existing aggregate
  ins Initiative
    fn mark_vip()
      # body...

  # Insert step into existing service
  ins CreateInitiative
    step acme_audit after persist
      AcmeSlackNotify.send!("created")

  # Refine: stock inlined, then extra work; return value explicit
  rfn CreateInitiative
    step
      init = stock
    step
      ret init

  # Replace: no stock
  rpl ArchiveInitiative
    step
      ret Ok

  # Remove from product surface
  omit SomeLegacyService

  # Rename for client branding / API surface
  ren ListInitiatives ListPrograms
```

---

## Merge algorithm (normative)

1. **Parse** leaf package; collect `adapt` edges → ordered base chain
   `[Base0, Adapter1, …, Leaf]`.
2. **Load** each package’s AST (full package, not expose-only).
3. **Seed** `Merged = clone(Base0)`.
4. For each package `P` in `Adapter1 … Leaf`:
   - Apply `P`’s patches in source order:
     - **`ren`**: rename symbol in `Merged`; rewrite references in `Merged`.
     - **`omit`**: remove path from `Merged`.
     - **`ins`**: insert children at path (+ step position).
     - **`rpl`**: replace body at path; reject if body contains `stock`.
     - **`rfn`**: replace body at path; expand every `stock` to **snapshot of
       body before this `rfn`** (hygienic rename of stock locals).
   - Then **merge new top-level items** from `P` (constructs/services not
     already present) into `Merged`.
5. **Emit** single `Solution` / package IR for check + codegen.
6. **Provenance map**: each final symbol → list of contributing package names
   (for IDE / diagnostics).

### Hygiene

- Locals from inlined `stock` get unique prefixes if they collide with the
  refining body’s names.
- Refining body may only observe **return value** of `stock` in expression
  form, not private locals of the ancestor (unless we later add an explicit
  export mechanism — **out of scope**).

### Multi-level

```text
stock at Regional  = body from wear_test (after Regional's earlier patches)
stock at Acme      = body from Regional merge (already includes wear_test inline)
```

Final `CreateInitiative` is one flat function.

---

## Check rules

| ID | Rule |
|----|------|
| ADP-C1 | `adapt X` resolves to package sources |
| ADP-C2 | Cannot `adapt` platform packages (`dlx_core`, engine crates) — allowlist or denylist |
| ADP-C3 | `ins`/`rfn`/`rpl`/`omit`/`ren` path must exist on base (after prior patches in this file) |
| ADP-C4 | `stock` only inside `rfn` body |
| ADP-C5 | `rpl` body must not contain `stock` |
| ADP-C6 | `ren` new name must not collide |
| ADP-C7 | Adapt graph: no cycles; diamonds error unless `adapt a, b order a then b` |
| ADP-C8 | `rfn`/`rpl` on `svc`/`fn`: input/output contract must match base (same params/return) unless explicit `rfn … with` later |
| ADP-C9 | After merge, `veil check` on flattened IR as today |

---

## IDE / dual-loop

| Surface | Behavior |
|---------|----------|
| File badge | “Adapts: wear_test → regional → (this)” |
| Graph | Default: **leaf view** (what ships). Toggle: **patches only** |
| Source dock | Optional **flattened** read-only view (serialize merged IR) |
| Diagnostics | Point at patch span in leaf file; mention base symbol |
| Agent | Prefer edit leaf patches; warn if editing base while leaf open |

---

## Codegen

No new target backend. **Input to `veil gen` is the merged Solution.**  
One workspace, one set of crates, one `CreateInitiative` body.

---

## Non-goals

- Runtime dynamic adapt / hot patch of running services  
- Adapting `.layer` files with the same ops (language stays `use layer`)  
- Per-initiative-row adapt packages as the primary model (use DB + Reaction binding; adapt is for **product lines**)  
- Silent AOP / aspect weaving without `stock` / `ins`  

---

## Example product line

```text
examples/wear_test.veil          stock Wear Test Application
clients/acme/acme_wear_test.veil
  adapt wear_test
  ren ListInitiatives ListPrograms
  ins CreateInitiative
    step acme_tag after persist
      ...
  rfn CreateInitiative
    step
      init = stock
    step
      ret init
```

---

## Implementation order

See stories **ADP-000 … ADP-012** in [`stories/150-package-adapt.md`](../stories/150-package-adapt.md).
