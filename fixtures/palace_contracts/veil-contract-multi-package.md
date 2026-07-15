# veil-contract-multi-package

**Type:** Concept  
**Summary:** Multi-package = several `.veil` into one `veil_bin`. Not multi-project IDE hub.

## Contract

- `[dev].packages = ["platform.veil"]` in `veil.toml` pulls siblings into gen workspace.
- Recipe: gen each with `--no-prune`, then `veil gen-harness product.veil platform.veil -o out`.
- Green fixture: `fixtures/multi_harness/` — memory adapters only.
- Multi-project hub (`veil serve --multi`) is a different feature.

## Example

```toml
[dev]
packages = ["platform.veil"]

[[targets]]
package = "product.veil"
target = "rust"
output = "generated/backend"
```

```bash
make fixture-multi-harness
```

**Source of truth:** `fixtures/multi_harness/README.md`, `docs/HARNESS.md`
