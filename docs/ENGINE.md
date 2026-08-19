# Engine rules — language implementers

Normative rules for changing the VEIL language pipeline. Agents and humans
authoring product code should read [LANGUAGE.md](./LANGUAGE.md) and
[BANG_CONTRACT.md](./BANG_CONTRACT.md) instead.

---

## Sugar changes hit three phases + one test (ACS-007)

**Any surface sugar** (operators, annotations, desugars, bang/try/Opt/Res
call-site behavior, layer statement sugar, stub policy that affects lowering)
must update **all** of the following in the **same PR**:

| Phase | Typical crates / paths |
|-------|------------------------|
| **Parser** | `crates/veil-parser` (lex/parse of the sugar) |
| **Typecheck / IR** | `crates/veil-ir` (typecheck, names, lower, diagnostics) |
| **Codegen** | `crates/veil-codegen` (target lowering, harness) |
| **Test** | At least one unit/integration test that fails without the full chain |

### Hard rules

1. **No codegen-only sugar PRs.** If lowering invents behavior the parser or
   typechecker does not understand, reviewers **reject**.
2. **No typecheck-only or parser-only** half-landings either — same PR or a
   stacked PR that lands together before anything depends on the sugar.
3. **Docs:** if the sugar is user-visible, update [LANGUAGE.md](./LANGUAGE.md)
   and/or the relevant contract page in the same PR (or immediately after with
   a linked follow-up that does not leave agents on the wrong law).

### Motivating incident

Bang call-site semantics drifted more than once:

1. **ACS-001 era:** codegen forced Opt→NotFound while typecheck still typed
   `Opt` — agents invented `.unwrap()` / `.is_some()` on values that were already
   forced to `T` in some paths. Alignment of parser + typecheck + codegen fixed it.
2. **ACS-010 flip:** typecheck/codegen moved to **portable bang** (Res only; Opt
   stays Opt) while Tier-0 and layer prompts still taught transitional
   force — agents thrashed between “never unwrap after `!`” and live
   `type_mismatch: expected T, found Opt`. Docs must flip with the engine (this rule).

Current law: [BANG_CONTRACT.md](./BANG_CONTRACT.md) (ACS-010 portable / SL-001).

### PR checklist (copy into review)

```markdown
### Language sugar (if applicable)

- [ ] Parser updated (or N/A — not a sugar/syntax change)
- [ ] Typecheck / IR updated (or N/A)
- [ ] Codegen updated (or N/A)
- [ ] Test covers the full chain (or N/A)
- [ ] User-facing docs/contract updated when behavior is public
```

Reject with: *“sugar must land parser + typecheck + codegen + test together
(see docs/ENGINE.md ACS-007).”*

---

## Related

- [BANG_CONTRACT.md](./BANG_CONTRACT.md) — bang / Opt / Res law
- [COMPILE_PIPELINE.md](./COMPILE_PIPELINE.md) — gen + cargo for agents
- [ARCHITECTURE.md](./ARCHITECTURE.md) — crate layout
- Epic: [stories/170-agent-complexity-shoreup.md](../stories/170-agent-complexity-shoreup.md) (ACS-007)
