# Bang / Opt / Res contract

Authoritative rules for fallibility and optionality in VEIL.  
**Agents and humans:** treat this as law. Parser, typecheck, and codegen must agree.

Related: [LANGUAGE.md](./LANGUAGE.md) · [AGENT.md](./AGENT.md) · [HARNESS.md](./HARNESS.md)

---

## Type formers (portable)

| VEIL | Meaning | Typical targets |
|------|---------|-----------------|
| `Opt<T>` | Value may be absent (Maybe) | `Option<T>`, `T?`, `T \| null` |
| `Res!` | Fallible, no payload | `Result<(), E>` |
| `Res!<T>` | Fallible, payload `T` | `Result<T, E>` |

These are **generic constructs**, not Rust-only. Error type `E` is target-specific
(today Rust dual-loop uses `DomainError`).

---

## Declaration: `name!(…)`

A **`!` after the method name** means the method is **fallible**:

```
find!(id: Id) -> Opt<WearTest>
save!(wt: WearTest)
```

Desugars for checking/codegen as:

| Written | Effective return |
|---------|------------------|
| `save!(…)` (no `->`) | `Res!` |
| `find!(…) -> Opt<T>` | `Res!<Opt<T>>` |
| `list!(…) -> List<T>` | `Res!<List<T>>` |

The method’s **lookup name** is without `!` (`find`); the bang only marks fallibility.

---

## Call site: current engine law (ACS-001)

When you write a **bang call** `repo.find!(id)` (the `!` is kept on the call AST):

1. **Res unwrap** — codegen emits try (Rust: `.await?`). Effective success type drops `Res!`.
2. **Opt → NotFound (port / trait methods)** — if the success type is `Opt<T>` / `Option<T>`,
   dual-loop Rust codegen also emits `.ok_or(DomainError::NotFound)?`, so the bound
   value has type **`T`**, not `Opt<T>`.

```
wt = wear_test_repo.find!(id)   # wt : WearTest  (current dual-loop Rust)
items = repo.list_by_tenant!(tid)  # items : List<WearTest> / Vec<…>
```

### Forbidden after bang (when result is forced to `T`)

Do **not** write:

```
existing = repo.find!(id)
guard existing.is_some(), "…"     # WRONG — not Opt anymore
wt = existing.unwrap()            # WRONG — not Option
```

**Correct:**

```
wt = wear_test_repo.find!(id)
wear_test_repo.save!(wt)
ret wt
```

### Non-bang calls

`repo.find(id)` (no `!`) does not apply call-site bang unwrap; prefer bang for fallible ports.

---

## Target policy vs language

| Concern | Language (portable) | Dual-loop Rust product policy |
|---------|--------------------|-------------------------------|
| Opt / Res types | Yes | Same |
| Decl `name!` | Fallible method | Same |
| Call bang = try Res | Portable idea | `.await?` |
| Call bang = Opt→NotFound | **Transitional product policy** | `.ok_or(NotFound)?` |

**Agents must follow the current engine law above** until migration (below) lands.

---

## ACS-010: portable bang (design choice)

### Decision (preferred end state)

| Call form | Meaning | Success type |
|-----------|---------|--------------|
| `repo.find!(id)` | **try / Res only** | after Res unwrap: still `Opt<T>` if declared `-> Opt<T>` |
| `require repo.find!(id)` (or layer default / annotation) | Force present | `T` (maps to NotFound / equivalent on target) |
| `repo.find(id)` | No bang unwrap | as declared |

**Rationale:** `Opt` and `Res!` stay generic constructs (Maybe / Result). Silencing
Opt→NotFound inside `!` couples the language to one product policy and hurts
multi-target honesty.

**Rejected for now:** a second glyph (e.g. `find!!`) — harder to teach; prefer
named `require` / annotation.

### Implementation plan + migration

| Phase | Work |
|-------|------|
| **1 — Contract (done)** | This section; Tier-0 notes transitional vs preferred |
| **2 — Typecheck flag** | `portable_bang` (or package/layer ann): bang strips **Res only** |
| **3 — Codegen** | NotFound only for `require` / `@force` / layer `opt_force_policy not_found` — **not** implicit on every bang |
| **4 — Migrate examples** | Replace `x = repo.find!(id)` used as `T` with `x = require repo.find!(id)` (or keep transitional bang until package opt-in) |
| **5 — Flip default** | After ladder + wear_test green under portable_bang, make preferred law the default |

**Before (transitional types):** `find!` → `T`  
**After (portable types):** `find!` → `Opt<T>`; `require find!` → `T`

### Tests

Unit tests in `veil-ir` document **current** call-site types
(`bang_call_strips_res_and_opt`). When phase 2 lands, add
`bang_call_portable_keeps_opt` and keep a transitional-mode test.

### Codegen NotFound policy

Target mapping for “force present” becomes:

- layer / package annotation (e.g. dual-loop default `not_found`), **or**
- explicit `require` lowering

Not an invisible side effect of every bang call.
---

## Golden example

```
port WearTestRepo
  find!(id: Id) -> Opt<WearTest>
  list_by_tenant!(tenant_id: Id) -> List<WearTest>
  save!(wear_test: WearTest)

handler HandleGetWearTest
  input
    id: Id
    @dep wear_test_repo: WearTestRepo
  step load
    wt = wear_test_repo.find!(id)
    ret wt

handler HandleListWearTests
  input
    tenant_id: Id
    @dep wear_test_repo: WearTestRepo
  step query
    items = wear_test_repo.list_by_tenant!(tenant_id)
    ret items
```

---

## Implementation notes (engine)

| Phase | Behavior |
|-------|----------|
| Parse | Keep `!` on call method (`find!`) |
| Typecheck | Bang call: strip `Res!` then `Opt` → `T` (matches dual-loop codegen) |
| Codegen | Port bang + Option: `.await?.ok_or(NotFound)?`; method name without `!` |

Sugar changes must update **parser + typecheck + codegen + test** in one PR
([ACS-007](./ENGINE.md#sugar-changes-hit-three-phases--one-test-acs-007) —
[`docs/ENGINE.md`](./ENGINE.md)).
