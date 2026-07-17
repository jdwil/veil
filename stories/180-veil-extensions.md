# VEIL Extensions — platform stories (EXT-*)

**Goal:** Runtime-managed mini-programs (Reactions, complex Signals, Activations,
UI panels) as normal VEIL packages: registry, publish, invoke/mount, stock +
custom authoring. Domain products store only `ExtensionRef(id, version, params)`.

**Status:** Backlog · design approved  
**Priority band:** P1 (domain + local runtime path), P2 (AWS adapters, UI mount)  
**Canonical design:** `/home/jd/dev/veil-projects/application/docs/veil-extensions.md`  
**Canonical stories (full acceptance text):**  
`/home/jd/dev/veil-projects/application/docs/veil-extensions-stories.md`  
**Platform pointer:** [`docs/EXTENSIONS.md`](../docs/EXTENSIONS.md)  
**Related:** [140 pure VEIL runtime](140-pure-veil-runtime.md), [80 runtime platform](80-runtime-platform.md),
[RuleEngine design](file:///home/jd/dev/veil-projects/application/docs/rule-engine-signal-trigger-reaction.md)

This file is the **platform tracking surface** for the epic. Story bodies live
in the application doc so product + runtime share one checklist; **runtime MUST
rules below apply to every veil-runtime-owned story** and override convenience
shortcuts.

---

## MUST — every veil-runtime Extensions story

### R1 — Fully VEIL-authored product code

| | |
|--|--|
| **MUST** | Extensions registry, publish, invoke/mount orchestration, catalog, fork, and any runtime **shell UI** for extensions are written in **VEIL** (`.veil` / `.layer` / `.stub` in the runtime product). |
| **MUST NOT** | Grow handwritten Rust/TS product logic for Extensions outside VEIL sources. |
| **Allowed residual** | Compiler/IDE engine (`veil-parser`, `veil-ir`, `veil-codegen`, `veil-server`) and thin trampoline — same residual as pure-runtime. Ports/adapters use stubs + SDKs; **declarations and wiring are VEIL**. |
| **Review** | Prefer “changed only `.veil`/`.layer`/`.stub`?” for Extensions features. |

### R2 — Ports + adapters (local first, AWS when deployed)

| | |
|--|--|
| **MUST** | External effects only via **ports**: meta registry, object/blob store, source VCS, compile/exec, artifact store. |
| **MUST** | **Local adapters** first-class for dual-loop and CI (filesystem / file-meta / local git or package dirs / local artifacts). |
| **MUST** | **AWS adapters** (DDB, S3, git-on-S3, optional ECR) implement the **same ports** for deploy; config-selected. |
| **MUST NOT** | Call AWS SDKs from handlers without a port; MUST NOT put AWS shapes in domain types. |
| **MUST** | Tests and default local run work **without** AWS credentials. |
| **SHOULD** | LocalStack only to validate AWS adapters — not daily path. |

### R3 — Products invoke; runtime loads

| | |
|--|--|
| **MUST** | application / wear_test call **runtime invoke/mount** — they do not load extension binaries themselves. |
| **MUST** | Capability allow-list at publish and invoke. |

Violating R1–R3 fails the story even if the feature “works” in a demo.

---

## Story index

| ID | Title | Owner | Pri | Stage |
|----|--------|-------|-----|-------|
| [EXT-01](file:///home/jd/dev/veil-projects/application/docs/veil-extensions-stories.md#ext-01-domain-extensionref-on-reaction) | Domain `ExtensionRef` on Reaction | application | P1 | E0 |
| [EXT-02](#ext-02) | Registry create/list/get version | **veil-runtime** | P1 | E1 |
| [EXT-03](#ext-03) | Publish + invoke ABI | **veil-runtime** | P1 | E2 |
| [EXT-04](file:///home/jd/dev/veil-projects/application/docs/veil-extensions-stories.md#ext-04-fire-path-resolves-extensionref--invoke) | Fire path → invoke | application + runtime | P1 | E2 |
| [EXT-05](file:///home/jd/dev/veil-projects/application/docs/veil-extensions-stories.md#ext-05-reactionlayer-palette-constructs) | `reaction.layer` palette | application / layers | P1 | E3 |
| [EXT-06](file:///home/jd/dev/veil-projects/application/docs/veil-extensions-stories.md#ext-06-embedded-ide-shell-palette--canvas--inspector) | Embedded IDE shell | wear_test + designkit | P1 | E3 |
| [EXT-07](#ext-07) | Agent palette gate | **veil-runtime** + product | P1 | E3 |
| [EXT-08](#ext-08) | Stock catalog + params | **veil-runtime** + wear_test | P1 | E4 |
| [EXT-09](#ext-09) | Duplicate stock → custom | **veil-runtime** + wear_test | P1 | E4 |
| [EXT-10](#ext-10) | Multi-scope catalogs | **veil-runtime** | P2 | E5 |
| [EXT-11](#ext-11) | AWS adapters (S3/DDB/git) | **veil-runtime** | P2 | E5 |
| [EXT-12](#ext-12) | UI mount + complex Signal rails | **veil-runtime** + products | P2 | E6 |
| EXT-00 | Epic DoD checklist | all | P1 | — |

Full acceptance criteria, checklists, and non-goals:  
**`application/docs/veil-extensions-stories.md`**.

Below: **runtime-owned** stories restated with R1–R2 MUST baked into acceptance
so platform implementers never miss them.

---

## EXT-02: Extension registry

**Status:** Done · **Priority:** P1 · **Owner:** veil-runtime  

**As a** platform  
**I want** VEIL-authored registry APIs with local ports  
**So that** `ExtensionRef` resolves to real packages without AWS  

**MUST:** R1, R2  

**Acceptance (summary):**

- [ ] Domain + handlers + services in **VEIL only** (product code)
- [ ] Ports for meta + source; **local adapters** default
- [ ] create / list / get / list versions; integer `current_version`
- [ ] Tests without AWS
- [ ] Adapter switch documented for future DDB/S3

---

## EXT-03: Publish + invoke

**Status:** Done · **Priority:** P1 · **Owner:** veil-runtime  

**As a** extension author  
**I want** publish → immutable int version and `invoke(id, version, ctx, params)`  
**So that** Fire runs pinned code  

**MUST:** R1, R2, R3  

**Acceptance (summary):**

- [ ] Publish orchestration in VEIL; compile/artifact behind ports
- [ ] Local gen+compile+artifact path works offline
- [ ] Stable invoke ABI; capability checks
- [ ] Products never load artifacts directly
- [ ] AWS artifact adapter later implements same port (EXT-11)

---

## EXT-07: Agent palette gate

**Status:** Todo · **Priority:** P1 · **Owner:** veil-runtime + product  

**MUST:** R1; server-side reject  

**Acceptance (summary):**

- [ ] Agent tools limited to active palette layers
- [ ] Unpresentable IR rejected
- [ ] Dual-loop proof of reject path

---

## EXT-08 / EXT-09: Stock + fork

**Status:** Todo · **Priority:** P1 · **Owner:** veil-runtime (+ wear_test UX)  

**MUST:** R1, R2  

**Acceptance (summary):**

- [ ] Stock catalog APIs in VEIL; seed packages on local disk
- [ ] Fork copies via source port; `created_from` lineage
- [ ] Local-only dual-loop E2E

---

## EXT-10: Multi-scope catalogs

**Status:** Todo · **Priority:** P2 · **Owner:** veil-runtime  

**MUST:** R1, R2  

**Acceptance (summary):**

- [ ] Platform | Product | Tenant scopes; configurable visibility
- [ ] Tenant isolation tests; promotion dev-gated

---

## EXT-11: AWS adapters

**Status:** Todo · **Priority:** P2 · **Owner:** veil-runtime  

**MUST:** R1, R2 — **local adapters remain default and complete**  

**Acceptance (summary):**

- [ ] DDB meta, S3 objects/artifacts, git-on-S3 source — same ports as local
- [ ] No domain type changes for AWS
- [ ] LocalStack or contract tests for AWS path
- [ ] One-repo-per-extension default

---

## EXT-12: UI mount ABI

**Status:** Todo · **Priority:** P2 · **Owner:** veil-runtime + products  

**MUST:** R1, R2, R3  

**Acceptance (summary):**

- [ ] `mount(id, version, slot, props)` via runtime
- [ ] FE artifacts local vs S3 via artifact port
- [ ] Complex Signal `impl` uses same invoke rails

---

## First slice (platform)

1. Support **EXT-01** contract from application (consume `ExtensionRef` shape).  
2. **EXT-02** VEIL registry + local ports.  
3. **EXT-03** publish + invoke local.  
4. Unblock application **EXT-04** fire wiring.  
5. Do **not** implement EXT-11 until ports in 02/03 are clean.

---

## Board

| ID | Status |
|----|--------|
| EXT-01 | Done |
| EXT-02 | Done |
| EXT-03 | Done |
| EXT-04 | Done |
| EXT-05 | Todo (application/layers) |
| EXT-06 | Todo (wear_test) |
| EXT-07 | Todo |
| EXT-08 | Todo |
| EXT-09 | Todo |
| EXT-10 | Todo |
| EXT-11 | Todo |
| EXT-12 | Todo |
| EXT-00 | Todo |
