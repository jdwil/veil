# VEIL Extensions (platform pointer)

| Field | Value |
|-------|-------|
| **Document** | Platform index for Extensions |
| **Status** | Design approved (planning) |
| **Date** | 2026-07-17 |

---

## What this is

**Extensions** are VEIL mini-programs (Reactions, complex Signals, Activation behaviors, custom UI panels) **compiled to normal targets** (Rust / TypeScript) and **managed by veil-runtime**: registry, git-on-S3 source, DDB meta, artifacts, load, sandbox, invoke/mount.

Product domains store only an **`ExtensionRef`** (`extension_id` + **integer version** + optional `params`). They never own a second interpreter or per-app loader.

## Canonical design

Full design (domain + runtime + authoring + catalogs):

> **Product / application doc (canonical):**  
> `/home/jd/dev/veil-projects/application/docs/veil-extensions.md`  
> (sibling hub project `application` under `VEIL_PROJECTS_DIR`)

Related platform docs:

| Doc | Relevance |
|-----|-----------|
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | Git on S3, meta cache, build service, artifacts on S3/ECR; VEIL has no storage knowledge |
| [`VCS_MODEL.md`](./VCS_MODEL.md) | Local disk/git for daily IDE; object store + meta for remote/deploy |
| [`IDE_RUNTIME.md`](./IDE_RUNTIME.md) | Runtime host embeds IDE/server capabilities |
| [`STORAGE.md`](./STORAGE.md) | Object storage adapters |
| [`COMPILE_PIPELINE.md`](./COMPILE_PIPELINE.md) | Gen + compile pipeline |

## Principles (short)

1. **Runtime owns code lifecycle**; products own **refs + wiring**.
2. **Integer pinned versions** on production invoke paths.
3. **Stock** (params forms) and **Custom** (embedded palette IDE) share one model; **duplicate-as-custom** with lineage is required.
4. **Catalog scopes** are configurable: platform ∪ product ∪ tenant.
5. **Agent ⊆ layer palette** — reject IR that cannot be fully presented in the configured IDE.
6. **Default: one git repo per extension package**; curated stock monorepos are a physical exception only.
7. **No second DSL** (no CEL/JSONLogic/Lark as the Reaction language).

## Ownership

| Layer | Responsibility |
|-------|----------------|
| **veil-runtime** | Registry, VCS, compile, artifacts, FFI/in-process/service invoke, mount |
| **application / products** | `ExtensionRef` on Reaction (etc.), bindings, embedded IDE shell |
| **Layers** | Node vocabulary, presentation, capability ports |

When implementation lands in-tree, link concrete packages and APIs from this page.

## Stories

| Location | Role |
|----------|------|
| `/home/jd/dev/veil-projects/application/docs/veil-extensions-stories.md` | Full EXT-01–12 acceptance criteria |
| [`stories/180-veil-extensions.md`](../stories/180-veil-extensions.md) | Platform tracking + **MUST** pure VEIL + ports/adapters |

**veil-runtime work MUST be VEIL-authored** (`runtime/src/runtime.veil`):

| In VEIL | Outside the engine |
|---------|-------------------|
| Domain, ports, application services | `.stub` crates for external APIs (`veil_local_fs`, aws-sdk-*) |
| **File* / Ddb* adapters** (full bodies) | Real crate behind the stub (`runtime/local_fs`) |
| Catalog, fork, seed, palette, publish, invoke | Bootstrap only **constructs** generated adapters (`extensions_deps`) |

**MISSION:** zero filesystem domain knowledge in `veil-codegen`. Adapters call `LocalFs` via stub, not `Fs.*` builtins.

## Related

- [`EXTENSIONS_AWS.md`](./EXTENSIONS_AWS.md) — EXT-11 Dynamo/S3 layout and migration
- [`EXTENSIONS_UI_SLOTS.md`](./EXTENSIONS_UI_SLOTS.md) — EXT-12 mount slots
- Smoke: `runtime/scripts/extensions_smoke.sh`
