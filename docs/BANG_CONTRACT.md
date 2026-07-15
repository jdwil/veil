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
| Call bang = Opt→NotFound | **Product policy** (ACS-010 may split) | `.ok_or(NotFound)?` |

Longer term (ACS-010): prefer bang = try only; explicit `require` / annotation for NotFound.
Until then, **current law above is what agents must follow**.

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
