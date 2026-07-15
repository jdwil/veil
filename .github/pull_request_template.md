## Summary

<!-- What does this PR do and why? -->

## Language sugar (ACS-007)

If this PR changes surface sugar (operators, bang/try/Opt/Res, layer statement
sugar, stub policy that affects lowering), **all** of the following must be
true. See [`docs/ENGINE.md`](../docs/ENGINE.md).

- [ ] **Parser** updated (or N/A — not a sugar/syntax change)
- [ ] **Typecheck / IR** updated (or N/A)
- [ ] **Codegen** updated (or N/A)
- [ ] **Test** covers the full chain (or N/A)
- [ ] **Docs/contract** updated when behavior is user-visible (or N/A)

Reviewers: reject **codegen-only** (or half-pipeline) sugar landings.

## Test plan

- [ ] `cargo test --workspace` (or scoped equivalent)
- [ ] Relevant fixtures if harness/agent-facing (`make fixture-ladder`, multi_harness, …)
