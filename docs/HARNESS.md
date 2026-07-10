# Authoring your own app harness (RT-000)

VEIL’s daily path is **VEIL-authored harnesses**, not eternal handwritten Rust
bootstrap. Use `@main` / `@pvd` / `@dep` (from `di.layer`) to compose a runnable
program.

## Minimal path (recommended demo)

```bash
# One-shot: generate + run real CreateItem handler (RT-003)
scripts/run_local_example.sh
```

Or manually:

```bash
veil gen examples/local_run.veil -o /tmp/local_run -t rust
cd /tmp/local_run && cargo run -p veil_bin
```

`@main` packages emit `crates/veil_bin` with a local harness that:

1. Constructs **`InProcessBus`** (from `veil_shared`, RT-001/004)
2. Wires **memory adapters** for ports
3. Calls a generated **application service** (not echo)

Library-only packages (no `@main`) generate context crates without `veil_bin`.

DI-only composition still works via `examples/di_example.veil` (`@pvd` / `@dep`).

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

## Layout (RT-021)

Generated workspaces with `@main` emit:

```text
Cargo.toml                 # workspace members include crates/veil_bin
crates/veil_shared/
crates/<context>/
crates/veil_bin/
  Cargo.toml               # [[bin]] name = "veil_bin"
  src/main.rs              # harness main
```

Verify: `cargo metadata --no-deps | jq '.packages[] | select(.name=="veil_bin")'`.

## VEIL packages for Bus / HTTP (RT-022)

- `layers/harness.layer` — prompts + patterns for `@main` composition
- `InProcessBus` generated into `veil_shared` when Bus is declared
- Minimal HTTP remains app-specific (axum in app or host); no eternal bootstrap

## `provided_by: "runtime"` (RT-023)

Local harness (`veil_bin`) supplies Bus (and allow-all Auth when declared)
without a handwritten host. Manifest still lists `provided_by: runtime` for
platform hosts; local gen emits concrete impls so `cargo run -p veil_bin` works.

## Known gaps

| Gap | Story |
|-----|--------|
| Empty adapter bodies still emit `todo!` | GEN-001/002 (flagged by escape diagnostics) |
| Full axum host package in pure VEIL | incremental RT-022 |

## Do not reintroduce

Permanent handwritten app harnesses in `runtime/bootstrap` as the only path.
Stage-0 may remain a thin `cargo` entry that calls generated `@main` only.

## Host modes (RT-002)

| Mode | Who wires deps | When |
|------|----------------|------|
| **App harness** | VEIL `@main` / `@pvd` constructs adapters | Local run, custom deploy |
| **Host harness** | External host reads `manifest.json`, injects `provided_by: "runtime"` | Shared platform |

`provided_by: "runtime"` in generated manifests means the host must supply
that trait (e.g. Bus). App mode needs no host if all deps are constructible.

## Related

- Product model: `stories/README.md`
- Runtime harness backlog: `stories/70-runtime-harness.md`
- Server API: `docs/SERVER.md`
- Agents (Rig): `docs/AGENT.md`
- ACP research: `docs/ACP_SPIKE.md`
