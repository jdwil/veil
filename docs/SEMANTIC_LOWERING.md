# Semantic lowering (codegen honesty)

How VEIL IR becomes target code **without lying**.  
Companion to [BANG_CONTRACT.md](./BANG_CONTRACT.md) and [ENGINE.md](./ENGINE.md).

---

## Goal

Agents must trust:

1. **What typecheck says** matches **what codegen emits**
2. **Green compile** is not purchased by constant-folding or dropping statements
3. **Cross-context** `invoke`/`request` resolve to real handlers when the package defines them

If a fix reintroduces “emit `true` so it typechecks,” it is a regression.

---

## Invariants (pin these)

| ID | Rule | Where enforced |
|----|------|----------------|
| **SL-001** | Bang strips `Res!` only; `Opt` stays `Opt` | typecheck + rust codegen |
| **SL-002** | `if cond then expr` does not require `else` | parser |
| **SL-003** | `ret null` on `Opt` → `Ok(None)`; on non-Opt → `Err(NotFound)` | codegen |
| **SL-004** | Bus `null` fields are JSON null, not the string `"null"` | `to_json_arg` |
| **SL-005** | Known `invoke Msg` decodes to domain type when return is known | codegen `bus_returns` |
| **SL-006** | `invoke`/`request` target must name a svc/tool in the package | typecheck `missing_handler` |
| **SL-007** | REST paths unique across multi-context harness (collision → `/api/{crate}/…`) | bin codegen |
| **SL-008** | `is_some`/`is_none` on non-`Opt` → error | typecheck `opt_method_on_non_opt` |

---

## Bus invoke typing

Desugar:

```
plan = invoke Reconcile{desired, environment, new_artifact_hash}
```

→ `Bus.invoke(Reconcile{…})` → JSON envelope →  
`serde_json::from_value::<ReconcileResult>(deps.bus.invoke(...).await?)?`  
when `Reconcile` is registered with return type `ReconcileResult`.

If the service returns `Json` (or unknown), leave `serde_json::Value`.

Handler registration in `veil_bin` must still implement the message name list
(`register_handlers` + real `bus.register` closures).

---

## Regression harness

| Test | Purpose |
|------|---------|
| `veil-ir` bang_* tests | Portable Opt law |
| `codegen_tests` `adapter_if_then_without_else_*` | Empty Opt returns |
| `codegen_tests` `rest_routes_pluralize_*` | branch → branches |
| `codegen_tests` `flow_return_type_from_bang_*` | bang keeps Option |
| `codegen_tests` `runtime_semantic_snapshots` | runtime.veil lowering pins |

Run:

```bash
cargo test -p veil-ir --lib bang_
cargo test -p veil-codegen --test codegen_tests
make pure-runtime-build   # if present: gen + cargo check runtime/generated
```

---

## Anti-patterns (do not reintroduce)

1. Constant-folding `is_some` → `true` because the local was force-unwrapped  
2. Dropping `if then` statements when `else` is missing  
3. Emitting handler **names** without registering closures  
4. Papering over type errors with `Default::default()` for table/bucket without env  
5. Hand-editing `runtime/generated` instead of engine + re-gen  

---

## When adding a fix

1. Name the invariant (or add one to this table)  
2. Typecheck (if it is a VEIL-level rule)  
3. Codegen  
4. Unit or snapshot test that fails if the lie returns  
5. Update this doc or BANG_CONTRACT if the law changes  
