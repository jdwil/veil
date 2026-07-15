# Complexity ladder (ACS-006)

Green packages agents (and humans) can copy. Complexity = composition of levels.

| Level | Path | Skills | CI |
|-------|------|--------|----|
| **L0** | [`l0/`](l0/) | ctx, port, handler, ret | yes |
| **L1** | [`l1/`](l1/) | CRUD + bang find/list/save, guards | yes |
| **L2** | [`l2/`](l2/) → [`../multi_harness/`](../multi_harness/) | multi-package, gen-harness | yes |
| **L3** | [`l3/`](l3/) | `.stub`, `@field`, `@env` | yes |
| **L4** | sketched | UI + proxy / presentation | not yet |
| **L5** | sketched | package adapt / remote deps | not yet |

## Run all green levels

```bash
make fixture-ladder
```

## L4 (sketch — not green yet)

- Svelte / UI package + backend routes
- Presentation layer + proxy to REST
- Dual target: rust backend + frontend gen
- Story: follow-on after ladder green

## L5 (sketch — not green yet)

- `adapt` / remote package dependency
- Versioned expose surface between products
- Story: follow-on; see [docs/ADAPT.md](../../docs/ADAPT.md)

## Related

- Bang contract: [docs/BANG_CONTRACT.md](../../docs/BANG_CONTRACT.md)
- Harness: [docs/HARNESS.md](../../docs/HARNESS.md)
- Multi-package fixture: [fixtures/multi_harness/](../multi_harness/)
- Epic: [stories/170-agent-complexity-shoreup.md](../../stories/170-agent-complexity-shoreup.md)
