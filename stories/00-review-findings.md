# Codebase Review Findings (2026-07-10)

Snapshot that motivated the current story backlog. Update this when a major
re-review lands; do not treat it as acceptance criteria (stories are).

## What works

| Area | Reality |
|------|---------|
| Parser / core expressions | Broad coverage (~34 expr kinds); examples parse |
| Layer system | Transitive `mt`, palette from layers, stacking proof (`crm` → `ddd`) |
| Rust codegen | Real service/adapter/saga/bus paths for DDD-shaped apps |
| Local IDE shell | Graph topology, drill-down, palette, property panel, live gen preview |
| Runtime as VEIL app | `runtime.veil` models Storage/Tools/Daemon/Exec domains |
| Bootstrap | Compiles; InProcessBus + HTTP `/bus/*` shell exists |

## Critical gaps vs mission

### Machine loop (agents)

1. **`veil check` is not a checker** — structural validate only; no types;
   no unresolved calls; validation errors do not fail the process; dumps IR.
2. **`diagnostics::analyze` is nearly empty** — only `requires_implementation`;
   not wired into CLI check.
3. **No target capability matrix** — unsupported constructs degrade silently
   (`todo!`, partial TS).
4. **Adapter / SDK bodies often `todo!`** — compiles, does nothing.

### Human loop (reviewers)

1. **`pkg` sources marked non-editable** under serve — primary artifacts read-only.
2. **No VEIL source / critical-body review surface** — `veilSource` fetched but
   unused; generated Rust panel is the drill-down.
3. **No structural diff** — cannot approve “what changed.”
4. **Expression edit chrome overbuilt and underwired** — opposite of mission
   priority (review first).
5. **Drop/delete not persisted** — canvas-only mutations.

### Integrity

1. **Serializer drops field annotations (`@dep`)** and mangles some control-flow
   bodies (`IfLet`/`WhileLet` placeholders) — edit→save can corrupt source.
2. **No round-trip tests.**

### Invariant debt (engine knows too much)

1. Magic annotation name `"dep"` in builder/codegen/templates.
2. Smart-constructor field-name heuristics in `rust.rs` / `typescript.rs`.
3. Bus/orchestrator JSON routing and `Handle*` conventions in engine.
4. DDD-shaped constraint algorithms in `validate.rs` (`crud_for_aggregate`, …).
5. TS Svelte emission keys off subkind strings `"Component"|"Page"|"Layout"`.

### Runtime

1. Bootstrap **does not read `manifest.json`** or call generated handlers (echo).
2. Storage adapters are mass `todo!("SQL: …")` for S3/DDB.
3. Exec wiring/parse/compile/deploy services are stubs.
4. UI VEIL not generated; API shapes disagree with bootstrap.
5. No dedicated runtime docs.

## Architecture tension (resolved in product decisions)

Runtime was both a **generic harness** and a **self-hosted platform**. Decisions:

1. **Harnesses are VEIL-authored** (`@main` / app composition). Handwritten
   bootstrap is stage-0 only. Manifest remains useful for *host* mode and
   deploys; it is not the only way to run an app.
2. **Daily driver = project-root `veil serve`** (IDE + agent soon). Local
   platform runtime (fs+sqlite) is an optional local build/platform environment.
3. **Cloud adapters are pluggable.** fs+sqlite is first-class local.
   LocalStack/AWS only validate the AWS path — not a universal “cloud local.”
4. **Source preview** is multi-target and secondary to VEIL review surfaces.

Stories in `70` / `80` / `30` (UX-028) encode these. Dual-loop P0s still first.
