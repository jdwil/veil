# Bootstrap — seed only (RT-005)

This directory is a **legacy stage-0 trampoline** for the full `veil-runtime`
self-hosting app. It is **not** the default app harness path.

## Prefer (product default)

```bash
# VEIL-authored @main + generated InProcessBus
scripts/run_local_example.sh
# or:
veil gen examples/local_run.veil -o /tmp/out -t rust
cd /tmp/out && cargo run -p veil_bin
```

See `docs/HARNESS.md` and `layers/harness.layer`.

## This folder

Handwritten `InProcessBus` + HTTP wiring for the large `runtime.veil` package
until that package is fully regenerated with the same RT-001 path. Do not grow
new product demos here — write VEIL + `@main` instead.
