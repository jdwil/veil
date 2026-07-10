# Local compile pipeline MVP (RT-013)

## Purpose

Heavier “compile this package” path for agents/platform — complements
project-root `veil serve` (IDE).

## MVP steps

```bash
# 1. Generate
veil gen path/to/pkg.veil -o "$OUT" -t rust   # or -t typescript

# 2. Target build
(cd "$OUT" && cargo build)                    # rust
# (cd "$OUT" && npm i && npx tsc --noEmit)     # ts when package.json present

# 3. Record result (future ArtifactMetadata)
# success | failure + logs + artifact path
```

## Env

| Variable | Meaning |
|----------|---------|
| `VEIL` | path to `veil` binary (default: `veil` on PATH) |
| `CARGO` | path to cargo |
| `VEIL_DATA_DIR` | artifact/log root |

## Failures

Must be structured (exit code + stderr capture). No silent success without a
real build step.

## Relation to RT-001

Local app demos use `scripts/run_local_example.sh` (`cargo run -p veil_bin`).
Compile-as-a-service wraps the same gen+build for multi-package hosts.
