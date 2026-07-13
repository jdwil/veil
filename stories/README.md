# VEIL User Stories

Living backlog derived from codebase review against [`MISSION.md`](../MISSION.md).

**Primary authors:** agents  
**Primary reviewers:** humans (topology + critical bodies)  
**Product requirement:** dual feedback loops — machine check *and* human structural review

## Product model (decisions)

| Piece | Decision |
|-------|----------|
| **Daily driver** | `veil serve` in a **project root**; product path embeds IDE in **runtime UX** |
| **Projects (runtime local)** | `~/.veil/config.json` + projects dir; first-run prompt; `veil init` / hub create; multi-project = **one** `veil-server` process ([120](120-projects-config-init.md), [IDE_RUNTIME.md](../docs/IDE_RUNTIME.md)) |
| **In-IDE agent** | Prompt → tools edit sources → IDE live-refresh; models + ACP/MCP pluggable ([100-ide-agent.md](100-ide-agent.md)) |
| **App harness** | **VEIL-authored** via `@main` / composition — not eternal handwritten bootstrap |
| **Local platform runtime** | Optional; **source on disk**, **metadata in sqlite**; cloud uses object store + meta DB |
| **Cloud** | Pluggable adapters per provider; LocalStack/AWS only for AWS path testing |
| **Source preview** | Multi-target, navigable, **secondary** to VEIL topology/body review |
| **`examples/`** | Syntax demos + CI — **not** the default IDE / runtime workspace |

## How to use

- Stories are acceptance-oriented. Implement in priority order unless blocked.
- IDs are stable; do not renumber — mark **Done** in the status line.
- Each story names **mission impact** so we do not confuse chrome with product.
- Prefer closing dual-loop stories before expression-editor chrome or new target demos.

## Priority bands

| Band | Meaning |
|------|---------|
| **P0** | Broken trust or unusable dual loop — do now |
| **P1** | Core mission path (check, review, harness, agent vertical slice) |
| **P2** | Local platform runtime / ACP+MCP versatility / multi-target honesty |
| **P3** | Cloud adapters, remote agent sessions, expressiveness parity marches |

## Index

| File | Theme |
|------|-------|
| [00-review-findings.md](00-review-findings.md) | Snapshot of issues found in review |
| [10-check-loop.md](10-check-loop.md) | Agent machine loop (`veil check`, diagnostics, types) |
| [20-serialize-edit.md](20-serialize-edit.md) | Round-trip integrity, edit API honesty |
| [30-viewer-review.md](30-viewer-review.md) | Human topology + critical-body review + source preview |
| [35-layer-presentation.md](35-layer-presentation.md) | **Layer-driven views / hierarchy / layout** (paradigm UX) |
| [40-viewer-restructure.md](40-viewer-restructure.md) | Persist structure edits, multi-file, navigation |
| [50-invariant-debt.md](50-invariant-debt.md) | Zero domain knowledge — purge engine heuristics |
| [60-codegen-targets.md](60-codegen-targets.md) | Codegen fidelity, capabilities, escape hatches |
| [70-runtime-harness.md](70-runtime-harness.md) | VEIL-authored harness + host/manifest modes |
| [80-runtime-platform.md](80-runtime-platform.md) | Local platform (fs+sqlite) + pluggable cloud |
| [90-parity-future.md](90-parity-future.md) | Expressiveness parity roadmap |
| [100-ide-agent.md](100-ide-agent.md) | In-IDE agent, models, ACP/MCP, SourceStore tools |
| [110-layer-dsl-ide.md](110-layer-dsl-ide.md) | **Layer / team-DSL IDE** — full-capability language designer loop |
| [120-projects-config-init.md](120-projects-config-init.md) | **Config first-run, `veil init`, projects hub, multi-project kernel** |
| [130-runtime-ux-audit.md](130-runtime-ux-audit.md) | **Runtime UX audit** — bootstrap vs multi-serve gaps (RTU-*) |
| [140-pure-veil-runtime.md](140-pure-veil-runtime.md) | **Pure VEIL runtime** — front+back VEIL, full product (PVR-*) |
| [141-pure-runtime-capability-gaps.md](141-pure-runtime-capability-gaps.md) | **What it takes** — CAP-001–007 engine gaps + sprint plan |
| [150-package-adapt.md](150-package-adapt.md) | **Package adapt** — specialize stock products (`adapt` / `ins` / `rfn` / `rpl` / `omit` / `ren`) |

## Suggested first slice (P0)

Original dual-loop P0/P1 path is largely **Done**. Historical order kept for
context:

1. **UX-010** — `pkg` files editable under `veil serve`  
2. **UX-011** — File select sets active file (API schema fix)  
3. **UX-020** — VEIL source / critical-body review pane  
4. **LAY-001 → LAY-003** — Layer presentation grammar, API, generic view switcher  
5. **LAY-004** — DDD model hierarchy as first proof  
6. **RT-000** — VEIL-authored `@main` harness proof  

## Next stack (open follow-ups)

Surfaced after closing the initial backlog (design/MVP honesty, harness gaps,
agent safety, cloud stubs). Prefer **P2 dual-loop trust** before more target demos.

### P0/P1 — language designer (team DSLs)

| ID | Theme | File |
|----|--------|------|
| **DSL-001–004** | Serve layers, edit, check, hot reload | [110](110-layer-dsl-ide.md) |
| **DSL-005–008** | Layer topology, palette, props, structured ops | [110](110-layer-dsl-ide.md) |
| **DSL-009–011** | Presentation/prompts, diff, agent layer tools | [110](110-layer-dsl-ide.md) |

### P0 — engine capabilities for pure runtime (next)

| ID | Theme | File |
|----|--------|------|
| **CAP-001–007** | **Done** — engine gaps for pure runtime (link, host, register_all, SPA, FS/Git, bin, config PATCH) | [141](141-pure-runtime-capability-gaps.md) |

### P1 — pure VEIL runtime product (after CAP-*)

| ID | Theme | File |
|----|--------|------|
| **PVR-010 / 021–023 / 032** | `@main` host; gen UI primary; quarantine hand HTML | [140](140-pure-veil-runtime.md) |
| **PVR-*** | Remaining product DoD polish | [140](140-pure-veil-runtime.md) |

### P1 — package adapt (product lines)

| ID | Theme | File |
|----|--------|------|
| **ADP-000–013** | `adapt` + path patches (`ins`/`rfn`/`rpl`/`omit`/`ren`) + flatten merge | [150](150-package-adapt.md) · [ADAPT.md](../docs/ADAPT.md) |

### P2 — trust & daily driver

| ID | Theme | File |
|----|--------|------|
| **PVR-012–014 / 016** | Git, compile, local deploy, agents | [140](140-pure-veil-runtime.md) |
| **PVR-040–041** | Registry + artifacts | [140](140-pure-veil-runtime.md) |
| **DSL-012–014** | Team consumer mode, scaffold, impact view | [110](110-layer-dsl-ide.md) |
| **PAR-015** | Spike capability honesty (signature vs body) | [90](90-parity-future.md) |
| **AGT-013** | Agent write path allowlist | [100](100-ide-agent.md) |
| **AGT-014** | Plan-only agent mode | [100](100-ide-agent.md) |
| **AGT-015** | Token budgets on `/api/context` | [100](100-ide-agent.md) |
| **AGT-017** | Remote structured EditOp | [100](100-ide-agent.md) |
| **RT-021–023** | Bin layout, Bus package, provided_by | [70](70-runtime-harness.md) |

### P3 — parity, cloud, polish

| ID | Theme | File |
|----|--------|------|
| **PAR-011 / 012** | Swift / Kotlin body lowering | [90](90-parity-future.md) |
| **PAR-013** | UI IR constructs + Svelte codegen | [90](90-parity-future.md) |
| **PAR-014** | Optional `@shared` marks | [90](90-parity-future.md) |
| **PAR-016** | Typed effect rows (only if needed) | [90](90-parity-future.md) |
| **RT-024 / 025** | Real DDB + SigV4 S3 | [80](80-runtime-platform.md) |
| **RT-026** | In-process HTTP (drop curl) | [80](80-runtime-platform.md) |
| **AGT-016 / 018** | Remote auth + live sync | [100](100-ide-agent.md) |
| **GEN-008 / 009** | Package multi-target + warning hygiene | [60](60-codegen-targets.md) |
| **DSL-015** | Many-layer workspace polish | [110](110-layer-dsl-ide.md) |

**Sequencing note:** Pure VEIL runtime needs **CAP-001–003 + CAP-005** first
([141](141-pure-runtime-capability-gaps.md)), then PVR host/UI purity. Prefer
**DSL-001–004** when language-designer iteration is the bottleneck. Close
**PAR-015** before expanding Swift/Kotlin demos. Prefer **AGT-013/014** before
multi-user remote auth.
