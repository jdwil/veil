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
