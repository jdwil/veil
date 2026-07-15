# Mind Palace durable contracts (ACS-009)

Five short **contract** pages for Mind Palace. Used when `MIND_PALACE=1` (seed via
agent) and as offline fixtures when palace is off in CI.

| Slug | Topic |
|------|--------|
| `veil-contract-bang-opt-res` | Bang / Opt / Res (ACS-001) |
| `veil-contract-dual-loop-smoke` | Dual-loop + smoke |
| `veil-contract-multi-package` | Multi-package `[dev].packages` (ACS-003) |
| `veil-contract-stubs` | Stubs / cargo_deps / harness_field |
| `veil-contract-routes` | `@route` + `list_routes` |

Each file: contract bullets + one example — not essays.

## Seed (palace on)

```bash
export MIND_PALACE=1
# … AWS env from docs/MIND_PALACE.md …
make serve PROJECT=…
# Agent dock or:
./scripts/seed_mind_palace.sh
```

Or send: **`seed mind palace`**. Host expands `SEED_MIND_PALACE_PROMPT` to create/update
these slugs (plus platform overview pages).

## Manual seed from fixtures

When wiki tools work, create/update each slug using the matching
`fixtures/palace_contracts/<slug>.md` body (summary + sections).

## Offline (palace off)

Agents still get Tier-0 pointers; humans read these files or
[docs/BANG_CONTRACT.md](../../docs/BANG_CONTRACT.md) /
[docs/HARNESS.md](../../docs/HARNESS.md).
