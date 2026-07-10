# VEIL User Stories

Living backlog derived from codebase review against [`MISSION.md`](../MISSION.md).

**Primary authors:** agents  
**Primary reviewers:** humans (topology + critical bodies)  
**Product requirement:** dual feedback loops — machine check *and* human structural review

## Product model (decisions)

| Piece | Decision |
|-------|----------|
| **Daily driver** | `veil serve` in the **project root** (IDE + agent panel) |
| **In-IDE agent** | Prompt → tools edit sources → IDE live-refresh; models + ACP/MCP pluggable ([100-ide-agent.md](100-ide-agent.md)) |
| **App harness** | **VEIL-authored** via `@main` / composition — not eternal handwritten bootstrap |
| **Local platform runtime** | Optional; **source on disk**, **metadata in sqlite**; cloud uses object store + meta DB |
| **Cloud** | Pluggable adapters per provider; LocalStack/AWS only for AWS path testing |
| **Source preview** | Multi-target, navigable, **secondary** to VEIL topology/body review |

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

## Suggested first slice (P0)

Check + serialize integrity are largely landed (CHK / SER). Next dual-loop path:

1. **UX-010** — `pkg` files editable under `veil serve`  
2. **UX-011** — File select sets active file (API schema fix)  
3. **UX-020** — VEIL source / critical-body review pane  
4. **LAY-001 → LAY-003** — Layer presentation grammar, API, generic view switcher  
5. **LAY-004** — DDD model hierarchy as first proof  

Then P1 harness proof: **RT-000** (document + run a VEIL-authored `@main` harness).

**Note:** Prefer **LAY-001–004** before deep hierarchical UX that would otherwise
hardcode DDD (aggregates, etc.) into the viewer.
