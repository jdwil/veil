# Bang / Opt / Res contract

Authoritative rules for fallibility and optionality in VEIL.  
**Agents and humans:** treat this as law. Parser, typecheck, and codegen must agree.

Related: [LANGUAGE.md](./LANGUAGE.md) · [AGENT.md](./AGENT.md) · [SEMANTIC_LOWERING.md](./SEMANTIC_LOWERING.md) · [ENGINE.md](./ENGINE.md)

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

## Call site: current engine law (ACS-010 portable)

When you write a **bang call** `repo.find!(id)` (the `!` is kept on the call AST):

1. **Res unwrap** — codegen emits try (Rust: `.await?`). Effective success type drops `Res!`.
2. **Opt is preserved** — if the success type is `Opt<T>` / `Option<T>`, it stays `Opt<T>`.
   Bang does **not** force absence to NotFound.

```
maybe = wear_test_repo.find!(id)   # maybe : Opt<WearTest>
items = repo.list_by_tenant!(tid)  # items : List<WearTest> / Vec<…>
```

### Force-present (Opt → T)

When the value **must** exist, force explicitly. Preferred surface (landed or landing):

```
wt = require repo.find!(id)   # wt : WearTest — absence → NotFound (or layer policy)
```

**Until `require` is available at every call site**, force with explicit Opt handling:

```
wt = repo.find!(id).unwrap()  # wt : WearTest — same intent as require
# or
maybe = repo.find!(id)
if maybe.is_none()
  ret null   # or err / early ret per handler policy
wt = maybe.unwrap()
```

| Call form | Meaning | Success type |
|-----------|---------|--------------|
| `repo.find!(id)` | try / Res only | `Opt<T>` if declared `-> Opt<T>` |
| `require repo.find!(id)` | force present | `T` (NotFound / layer policy) |
| `repo.find!(id).unwrap()` | force present (interim) | `T` |
| `repo.find(id)` | no bang unwrap | as declared |

**Rejected:** a second glyph (e.g. `find!!`) — harder to teach; prefer named `require` / layer policy.

### Soft absence is valid after bang

After `find!`, the bound value is still `Opt` when declared that way. These are **allowed**:

```
maybe = repo.find!(id)
if maybe.is_some()
  # …
guard maybe.is_some(), "optional path"   # only when you still hold Opt
x = maybe.unwrap()
```

### Hard “must exist then use as T”

```
wt = require repo.find!(id)   # or .unwrap() until require is universal
wear_test_repo.save!(wt)
ret wt
```

### Non-bang calls

`repo.find(id)` (no `!`) does not apply call-site bang Res unwrap; prefer bang for fallible ports.

---

## Target policy vs language

| Concern | Language (portable) | Dual-loop Rust product |
|---------|---------------------|------------------------|
| Opt / Res types | Yes | Same |
| Decl `name!` | Fallible method | Same |
| Call bang = try Res | **Yes (default)** | `.await?` |
| Call bang = Opt→NotFound | **No** — not part of `!` | Only via `require` / `.unwrap()` / layer force policy |

Layer/package may add default force-present for dual-loop presentation handlers
(`@opt_force not_found` or equivalent). That is **product policy**, not language sugar on every `!`.

---

## Golden example

```
port WearTestRepo
  find!(id: Id) -> Opt<WearTest>
  list_by_tenant!(tenant_id: Id) -> List<WearTest>
  save!(wear_test: WearTest)

# Soft: absence is data (return Opt; harness may map None → 404)
handler HandleGetWearTest
  input
    id: Id
    @dep wear_test_repo: WearTestRepo
  step load
    wt = wear_test_repo.find!(id)   # Opt<WearTest>
    ret wt

# Hard: must exist before mutation
handler HandleUpdateWearTest
  input
    id: Id
    @dep wear_test_repo: WearTestRepo
  step load
    wt = require wear_test_repo.find!(id)   # WearTest
    # … mutate wt …
    wear_test_repo.save!(wt)
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

## Historical note (ACS-001 transitional — **obsolete**)

An earlier dual-loop product policy folded Opt→NotFound into every bang call
(`find!` → `T` with silent `.ok_or(NotFound)?`). That coupled the language to one
error model and taught agents “never `.unwrap()` / `.is_some()` after `!`.”

**That law is no longer the engine default.** Typecheck and codegen use portable
bang (SL-001). Docs, Tier-0, layer prompts, and Mind Palace must not re-teach
transitional force.

Transitional unwrap helpers may remain in tests for comparison only
(`unwrap_bang_return_transitional`).

---

## Implementation notes (engine)

| Phase | Behavior |
|-------|----------|
| Parse | Keep `!` on call method (`find!`) |
| Typecheck | Bang call: strip `Res!` only; `Opt` stays `Opt` (ACS-010 / SL-001) |
| Codegen | Port bang: `.await?` only; **no** automatic `.ok_or(NotFound)?` on every bang+Opt |
| Force | `require` and/or `.unwrap()` / layer policy emit NotFound when present |

Sugar changes must update **parser + typecheck + codegen + test** in one PR
([ACS-007](./ENGINE.md#sugar-changes-hit-three-phases--one-test-acs-007) —
[`docs/ENGINE.md`](./ENGINE.md)).

### Related invariants

See [SEMANTIC_LOWERING.md](./SEMANTIC_LOWERING.md):

- **SL-001** — Bang strips `Res!` only; `Opt` stays `Opt`
- **SL-008** — `is_some` / `is_none` on non-`Opt` → error
