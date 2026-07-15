# Multi-package dual-loop fixture (ACS-003)

**What this is:** product + platform `.veil` packages generated into **one**
workspace and combined with `veil gen-harness` (local multi-package harness).

**What this is not:** multi-project IDE hub (`veil serve --multi`).

## Layout

| File | Role |
|------|------|
| `product.veil` | Product context (widgets) |
| `platform.veil` | Platform slice (notes) |
| `veil.toml` | `[dev].packages` + backend target |

Memory adapters only — no Dynamo/sqlx.

## CLI (CI recipe)

From repo root (requires `target/debug/veil` or `veil` on PATH):

```bash
OUT=/tmp/veil-multi-harness
rm -rf "$OUT"
VEIL="${VEIL_BIN:-target/debug/veil}"

$VEIL check fixtures/multi_harness/product.veil
$VEIL check fixtures/multi_harness/platform.veil

$VEIL gen fixtures/multi_harness/product.veil -o "$OUT" -t rust --no-prune
$VEIL gen fixtures/multi_harness/platform.veil -o "$OUT" -t rust --no-prune
$VEIL gen-harness \
  fixtures/multi_harness/product.veil \
  fixtures/multi_harness/platform.veil \
  -o "$OUT"

cd "$OUT" && cargo check -p veil_bin
```

Or: `make fixture-multi-harness`

## Dual-loop

```bash
make serve PROJECT=$PWD/fixtures/multi_harness
```

Backend gen uses `[dev].packages` so both packages land in `generated/backend`.
