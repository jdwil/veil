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
| [160-agent-runtime-observability.md](160-agent-runtime-observability.md) | **Agent runtime observability** — logs, generated code, HTTP probe, harness teaching (AGT-020–028) |
| [170-agent-complexity-shoreup.md](170-agent-complexity-shoreup.md) | **Agent complexity shore-up** — bang contract, multi-package fixture+CI, ladder, pipeline rule (ACS-001–012) |

## Suggested first slice (P0)

Original dual-loop P0/P1 path is largely **Done**. Historical order kept for
context:

1. **UX-010** — `pkg` files editable under `veil serve`  
2. **UX-011** — File select sets active file (API schema fix)  
3. **UX-020** — VEIL source / critical-body review pane  
4. **LAY-001 → LAY-003** — Layer presentation grammar, API, generic view switcher  
5. **LAY-004** — DDD model hierarchy as first proof  
6. **RT-000** — VEIL-authored `@main` harness proof  

## Board status (2026-07-13)

**Primary dual-loop + pure-runtime product path is closed.** Do not treat this
file’s older “next stack” narrative as open work — check each epic’s status
board (or `**Status:**` lines) as source of truth.

| Epic | Status | File |
|------|--------|------|
| Dual-loop core (check, serialize, viewer, layers, harness) | **Done** | [10](10-check-loop.md)–[70](70-runtime-harness.md) |
| **DSL-001–015** language designer IDE | **Done** | [110](110-layer-dsl-ide.md) |
| **CAP-001–007** engine gaps | **Done** | [141](141-pure-runtime-capability-gaps.md) |
| **PVR-000–041** pure VEIL runtime | **Done** | [140](140-pure-veil-runtime.md) |
| **PVR-032** hand shell quarantine | **Done** | `static/legacy/` + smoke guard |
| **PVR-042** remote multi-tenant | **Done (deferred)** | non-goal for pure-local; AGT remote paths exist |
| **ADP-000–013** package adapt | **Done** | [150](150-package-adapt.md) · [ADAPT.md](../docs/ADAPT.md) |
| **AGT-*** in-IDE agent | **Done** | [100](100-ide-agent.md) |
| **AGT-020–028** agent runtime observability | **Done** | [160](160-agent-runtime-observability.md) |
| **ACS-001–012** agent complexity shore-up | **In progress** (001–006 Done) | [170](170-agent-complexity-shoreup.md) |
| **RT-021–023** harness layout / bus / provided_by | **Done** | [70](70-runtime-harness.md) |
| Projects hub / multi-project kernel | **Done** | [120](120-projects-config-init.md) · [130](130-runtime-ux-audit.md) |

### Optional future (not blocking product DoD)

These may still appear as long-horizon spikes in their files; they are **not**
open dual-loop debt:

| Area | Notes | File |
|------|--------|------|
| Multi-target expressiveness | Swift/Kotlin depth, UI IR, effects | [90](90-parity-future.md) |
| Cloud adapters | Real DDB / SigV4 S3 beyond local path | [80](80-runtime-platform.md) |
| Codegen package multi-target polish | GEN-008/009 | [60](60-codegen-targets.md) |

New work should open a new story ID — do not re-open Done boards without a
regression.
