# Pure VEIL runtime (release goal)

Canonical story epic: [`stories/140-pure-veil-runtime.md`](../stories/140-pure-veil-runtime.md).

## Residual non-VEIL (allowed)

| Component | Role |
|-----------|------|
| `veil-parser` / `veil-ir` / `veil-codegen` | Language engine |
| `veil-server` | Dual-loop IDE HTTP kernel (multi-project) |
| `veil-local` | Storage adapters |
| Optional trampoline | ≤50 lines until `@main` host is complete |

Product domain + shell UI must be authored in `.veil` / `.layer` / `.stub`.

## Definition of done (D0–D4)

See story file. Human demo:

```bash
make pure-runtime
# open http://127.0.0.1:8080/
# create project → Open IDE → dual-loop works
```

## Capability matrix

See [`RUNTIME_CAPABILITIES.md`](RUNTIME_CAPABILITIES.md).

## What it takes to finish

Functional multi-host exists; **pure VEIL authorship** is blocked on engine
capabilities:

| CAP | Need |
|-----|------|
| CAP-001 | Link external crates (`veil-server`) from VEIL packages |
| CAP-002 | HttpHost port — mount IDE + static + listen from VEIL |
| CAP-003 | Generated Bus `register_all` |
| CAP-005 | SPA **bundle** emit for `runtime-ui.veil` |
| CAP-004/006 | FS/Git ports; multi-crate bin layout |

Full plan + sprint order: [`stories/141-pure-runtime-capability-gaps.md`](../stories/141-pure-runtime-capability-gaps.md).

**Estimate:** ~5–8 weeks focused (one senior) after CAP P0s land.
