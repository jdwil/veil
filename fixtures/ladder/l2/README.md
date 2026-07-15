# L2 — Multi-package harness

**Skills:** `[dev].packages`, `gen-harness`, dual-package `veil_bin`.

**Canonical fixture:** [`fixtures/multi_harness/`](../../multi_harness/) (ACS-003).

Multi-package local harness ≠ multi-project IDE hub.

## DO

- Product + platform `.veil` packages
- `veil.toml` `[dev].packages = ["platform.veil"]`
- Gen both with `--no-prune`, then `veil gen-harness …`
- Memory adapters only for the green CI path

## DON'T

- Confuse with `veil serve --multi` (IDE hub)
- Prune the workspace between package gens
- Expect one package gen to invent the sibling crate

## Verify

```bash
make fixture-multi-harness
# same as make fixture-ladder-l2
```

Full recipe: [multi_harness README](../../multi_harness/README.md).
