# Authoring your own app harness (RT-000)

VEIL’s daily path is **VEIL-authored harnesses**, not eternal handwritten Rust
bootstrap. Use `@main` / `@pvd` / `@dep` (from `di.layer`) to compose a runnable
program.

## Minimal path

1. Author a package with DI annotations, e.g. `examples/di_example.veil`:
   - `@pvd T` on a factory `fn` that builds type `T`
   - `@dep` on struct fields that must be injected
   - `@main` on a composition `fn` whose steps become the generated main
2. Generate and run:

```bash
veil gen examples/di_example.veil -o /tmp/di_out -t rust
cd /tmp/di_out && cargo build
# @main contributors emit crates/veil_bin (RT-001b):
cargo run -p veil_bin
```

Library-only packages (no `@main`) generate context crates without `veil_bin`.

3. Check dual-loop quality:

```bash
veil check examples/di_example.veil
```

## What already works

| Capability | Status |
|------------|--------|
| `@main` composition into generated main | Working via `di.layer` + rust codegen |
| `@dep` / `@pvd` DI graph | Working (INV-001: role:dependency) |
| Multi-context Bus orchestration | Opt-in when steps have `ctx` refs **and** layer defines routing traits (INV-003) |
| Smart constructors / timestamps | `rust.layer` `constructor_policy` (INV-002) |

## Known gaps

| Gap | Story |
|-----|--------|
| Dedicated runnable bin crate for every multi-context workspace | RT-001b |
| Bus + HTTP + handler registration fully in VEIL declare | RT-001 |
| InProcessBus as default local topology | RT-004 |
| Host-injected `provided_by: runtime` mode | RT-002 |
| Empty adapter bodies still emit `todo!` | GEN-001/002 (flagged by escape diagnostics) |

## Do not reintroduce

Permanent handwritten app harnesses in `runtime/bootstrap` as the only path.
Stage-0 may remain a thin `cargo` entry that calls generated `@main` only.

## Related

- Product model: `stories/README.md`
- Runtime harness backlog: `stories/70-runtime-harness.md`
- Server API: `docs/SERVER.md`
