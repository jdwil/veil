# Agent complexity shore-up — stable contracts, green compose path, curriculum

**Goal:** Make it reliable for in-IDE agents (and humans) to build **arbitrarily
complex** VEIL systems by composing stable patterns — not by fighting sugar
desync, stale dual-loop, or missing multi-package fixtures.

**Status:** In progress · P1  

**Depends on:** Agent observability ([160](160-agent-runtime-observability.md)),
multi-package harness / dual-loop ([70](70-runtime-harness.md), `[dev].packages`),
bang typecheck alignment (in-tree)  
**Mission impact:** Agents are primary authors. Complexity = composition of green
patterns + a closed edit→verify loop. Without contracts + fixtures + CI, agents
thrash on wear_test-scale products.

**Related**

- Observability tools: [160](160-agent-runtime-observability.md)  
- Language surface: [docs/LANGUAGE.md](../docs/LANGUAGE.md)  
- Harness / dual-loop: [docs/HARNESS.md](../docs/HARNESS.md)  
- Agent SOP: [docs/AGENT.md](../docs/AGENT.md)  
- Invariant / zero domain knowledge: [50](50-invariant-debt.md)

**Non-goals**

- Full redesign of Opt/Res/bang in this epic (contract + optional later split)  
- Multi-project IDE hub (already Done — [120](120-projects-config-init.md))  
- Unscoped shell for agents  
- Browser/network CDP  

**Vocabulary**

| Term | Meaning |
|------|---------|
| **Multi-package dev** | Several `.veil` packages into one gen workspace + `gen-harness` `veil_bin` |
| **Multi-project** | Several product roots in IDE hub — **not** this epic’s fixture |
| **Bang contract** | Written law for `!` / `Opt` / `Res!` on decl vs call |
| **Complexity ladder** | L0–L5 fixtures agents can copy as patterns |

---

## Problem

Agents fail on complex systems because:

1. **Overloaded bang** — `find!` means fallible method *and* try *and* Opt→NotFound; agents invent `.unwrap()` / `.is_some()`.
2. **No green multi-package template** — wear_test+dlx is the real goal but too heavy; compose path rots without a minimal CI fixture.
3. **Observe tools exist but habits don’t** — smoke/logs/routes/HTTP exist; agents still thrash without mandatory SOP / optional auto-restart.
4. **Path invention** — name-derived REST + missing `@route` on examples.
5. **No curriculum** — no L0→L5 green ladder for “compose complexity.”
6. **Sugar split-brain** — parser / typecheck / codegen diverge without a hard PR rule.
7. **Teaching scatter** — contracts not concentrated for wiki/Tier-0.

---

## Epic outcomes

1. Bang/Opt/Res **contract** is documented and injected into agent Tier-0.
2. **Green multi-package fixture** + CI (`check` → gen both → harness → `cargo check`).
3. Agent **closed loop** is mandatory in Tier-0; optional auto-restart after smoke.
4. Stock/public handlers use **`@route`**; name-derived routes are fallback-only in docs.
5. **L0–L3** (at least) fixtures green in CI; L4–L5 sketched.
6. **Pipeline rule**: sugar changes update parser + typecheck + codegen + test together.
7. Mind Palace / wiki: five **durable contract** pages (if palace enabled).

---

## Stories

### ACS-001: Bang / Opt / Res contract in LANGUAGE.md — Done · P0

**As a** human or agent author  
**I want** one authoritative page for `!`, `Opt`, and `Res!`  
**So that** call-site types match codegen and we stop inventing `.unwrap()`

**Acceptance criteria:**

- [x] New section (or short page linked from LANGUAGE.md) **Bang / Opt / Res contract**
- [x] Declares:
  - `name!(…)` on **declaration** = fallible method (return wrapped as `Res!` / `Res!<T>`)
  - `Res!` / `Res!<T>` / `Opt<T>` type meanings and target mappings (table)
  - **Call-site law for current engine** (document truth, not aspiration): bang call unwraps Res via try and, for port `Opt` returns, NotFound unwrap — **or** document the post-ACS-010 rule if that lands first
  - **Forbidden** after bang: `.unwrap()`, `.is_some()`, `.is_none()` on the bound result when call already forces `T`
- [x] One golden example: list + find + save (matches ddd handler pattern)
- [x] Cross-link from HARNESS.md and AGENT.md
- [x] Note portability: Opt/Res are generic; bang-call Opt→NotFound is product policy

**Depends:** current typecheck/codegen bang behavior  
**Mission impact:** Single source of truth for the scariest sugar  
**Touch:** `docs/LANGUAGE.md`, maybe `docs/BANG_CONTRACT.md`, links in AGENT/HARNESS

---

### ACS-002: Tier-0 injects bang contract + closed-loop SOP — Done · P0

**As an** IDE agent  
**I want** Tier-0 to state bang law and mandatory verify steps  
**So that** every turn starts with the same non-negotiables

**Acceptance criteria:**

- [x] `agent_context` TIER0 + TIER0_ACP include:
  - 6–10 lines from ACS-001 (bang / no unwrap after `find!`)
  - Closed loop: after backend HTTP edits → smoke → `dev_logs` on reject → `list_routes` → `dev_restart` → `http_request`
  - Prefer `@route`; do not invent paths
- [x] On WRITE REJECTED: must call `dev_logs` / `smoke_status` before large rewrite (wording)
- [x] ddd.layer prompt stays aligned (already partial — reconcile with ACS-001)
- [x] Budget-safe: no huge dump; pointer to LANGUAGE section if truncated

**Depends:** ACS-001, [160](160-agent-runtime-observability.md) tools  
**Mission impact:** Agents inherit the contract without wiki  
**Touch:** `agent_context.rs`, `layers/ddd.layer`

---

### ACS-003: Green multi-package dual-loop fixture + CI — Done · P0

**As a** developer or agent  
**I want** a minimal multi-**package** example that always compiles  
**So that** compose-via-harness is a copyable pattern (not multi-project hub)

**Acceptance criteria:**

- [x] Fixture tree, e.g. `fixtures/multi_harness/` or `examples/multi_harness/`:
  - `platform.veil` — one context, port + find/list/save (memory adapter preferred; no Dynamo/sqlx required)
  - `product.veil` — product handlers/routes (may call platform ports via co-hosted API or local use as appropriate)
  - `veil.toml` — `[[targets]]` rust backend + `[dev].packages` pointing at platform
- [x] Documented CLI recipe:
  ```bash
  veil gen product.veil -o out --no-prune
  veil gen platform.veil -o out --no-prune
  veil gen-harness product.veil platform.veil -o out
  cd out && cargo check -p veil_bin
  ```
- [x] CI job (or make target + CI step) runs check + gen + harness + `cargo check`
- [x] README in fixture: “multi-package local harness ≠ multi-project IDE hub”
- [x] Dual-loop start against fixture succeeds on a clean machine (smoke test notes)

**Depends:** multi-package gen / `--no-prune` / gen-harness (in-tree)  
**Mission impact:** Compose path can’t rot silently  
**Touch:** `fixtures/` or `examples/`, CI config / Makefile, HARNESS.md link

**Clarification:** This is **multi-package dev** (several `.veil` → one `veil_bin`), **not** multi-project hub.

---

### ACS-004: Closed-loop enforcement — auto-restart after successful smoke — Done · P1

**As an** IDE agent  
**I want** the running backend to load new gen after a good smoke  
**So that** `http_request` isn’t probing a stale binary

**Acceptance criteria:**

- [x] After **successful** smoke for a Rust target that is **owned** Running:
  - either auto `dev_restart`, **or**
  - hard Tier-0 + tool hint that `dev_restart` is required (if auto deferred)
- [x] Preference: **auto-restart once** for owned processes; attached externals → message only
- [x] Logs: `[dev] restart after smoke`
- [x] `VEIL_AGENT_AUTO_RESTART=0` escape hatch
- [x] Manual test: change `@route` → smoke OK → HTTP sees new path without human restart

**Depends:** ACS-002, smoke gate, `dev_restart` ([160](160-agent-runtime-observability.md))  
**Mission impact:** Kills silent 404 / stale process loops  
**Touch:** `devloop.rs`, env docs in AGENT.md

---

### ACS-005: `@route` on all stock public handlers; name-derived is fallback — Done · P1

**As an** agent  
**I want** examples and stock packages to declare routes explicitly  
**So that** `list_routes` matches author intent and I stop inventing paths

**Acceptance criteria:**

- [x] Every public HTTP handler/svc in stock examples + wear_test product has `@route("METHOD /path")`
- [x] HARNESS.md: `@route` authoritative; List/Get/Create name rules = **fallback only**
- [x] Tier-0: prefer `@route`; never invent paths without `list_routes`
- [x] Optional: warn in check when handler looks HTTP-facing but has no `@route` (P2 if noisy)

**Depends:** AGT-026 `@route` in harness  
**Mission impact:** Routes are data, not English heuristics  
**Touch:** examples, product packages, HARNESS.md, agent_context

---

### ACS-006: Complexity ladder L0–L3 green fixtures — Done · P1

**As an** agent (or human learning VEIL)  
**I want** a ladder of green packages from hello to multi-package  
**So that** complex systems are composition of known levels

**Acceptance criteria:**

| Level | Fixture | Skills |
|-------|---------|--------|
| L0 | hello svc + memory repo | ctx, port, handler, ret |
| L1 | CRUD + bang find/list/save | Opt/Res bang, guards (no unwrap after !) |
| L2 | multi-package harness (ACS-003) | `[dev].packages`, gen-harness |
| L3 | one stub SDK + adapter | stub, @field/@env |

- [x] Each level: `veil check` + `veil gen` + `cargo check` (L2 uses multi recipe)
- [x] Short DO/DON’T (≤15 lines) per level in fixture README or layer prompt
- [x] CI runs L0–L3
- [x] L4 (UI + proxy) and L5 (adapt) **sketched** as stories/stubs, not necessarily green yet

**Depends:** ACS-003 for L2  
**Mission impact:** Complexity = ladder, not one giant product  
**Touch:** `fixtures/ladder/` or `examples/ladder/`, CI

---

### ACS-007: Pipeline rule — sugar changes hit three phases + test — Done · P1

**As a** language implementer  
**I want** a hard rule that sugar never lands half-implemented  
**So that** bang/Opt-class bugs don’t recur

**Acceptance criteria:**

- [x] Doc in CONTRIBUTING or `docs/ENGINE.md`: any sugar change updates **parser + typecheck + codegen + one test** in the same PR
- [x] Checklist snippet for PR template (if repo uses one)
- [x] Reference the bang desync as the motivating incident
- [x] Reviewers reject “codegen-only” sugar PRs

**Depends:** none  
**Mission impact:** Prevents three-brain language  
**Touch:** docs, optional PR template

---

### ACS-008: Structured check diagnostics for agents — Done · P2

**As an** IDE agent  
**I want** `veil_check` / tool output to be machine-friendly  
**So that** I fix spans instead of rewriting whole files

**Acceptance criteria:**

- [x] Tool or flag returns structured items: `{ code, severity, message, span?, hint? }` (JSON) in addition to or instead of prose
- [x] ACP/Rig path can surface codes in tool result
- [x] At least type_mismatch + parse errors include span when available
- [x] Docs: how agents should consume structured diagnostics

**Depends:** existing check diagnostics  
**Mission impact:** Targeted fix loops  
**Touch:** `rig_tools` / MCP `veil_check`, maybe check formatter

---

### ACS-009: Mind Palace durable contracts (five pages) — Done · P2

**As an** agent with Mind Palace enabled  
**I want** five short contract pages  
**So that** platform questions hit stable truth first

**Acceptance criteria:**

- [x] Pages (or equivalent fixtures if palace off in CI):
  1. Bang / Opt / Res (ACS-001)
  2. Dual-loop + smoke
  3. Multi-package `[dev].packages` (ACS-003)
  4. Stubs / cargo_deps
  5. `@route` + list_routes
- [x] Each page: contract bullets + one example — not essays
- [x] Tier-0: wiki_search these topics when MIND_PALACE=1
- [x] Seed script or documented manual seed

**Depends:** ACS-001, ACS-003, Mind Palace optional path  
**Mission impact:** Durable memory of contracts  
**Touch:** palace seed, AGENT/MIND_PALACE docs

---

### ACS-010: Bang call semantics — portable split (optional evolution) — Todo · P2

**As a** multi-target language designer  
**I want** bang to stop silently meaning Opt→NotFound  
**So that** Opt/Res stay generic constructs (Maybe/Result)

**Acceptance criteria:**

- [ ] Design choice recorded (update ACS-001):
  - **Preferred direction:** bang call = try/Res only; Opt stays Opt unless explicit `require` / guard / second operator
  - **Or** explicit second glyph for force-Opt
- [ ] Implementation plan + migration for wear_test / examples
- [ ] Codegen NotFound policy becomes annotation or layer default, not implicit in `!`
- [ ] Tests for call-site types before/after
- [ ] Tier-0 and ddd.layer updated

**Depends:** ACS-001  
**Mission impact:** Grammar hygiene; multi-target honesty  
**Touch:** typecheck, codegen, LANGUAGE, fixtures

**Note:** Do **not** block ACS-001–007 on this. Contract-first; evolve second.

---

### ACS-011: list_routes from IR (pre-gen) — Todo · P2

**As an** agent  
**I want** intended routes even when gen failed  
**So that** I’m not blind after a bad write

**Acceptance criteria:**

- [ ] `list_routes` (or sibling tool) can derive from package IR: `@route` first, else name-derived fallback
- [ ] Flag or mode: `source=ir|generated` (default generated when present)
- [ ] Matches harness policy when both available
- [ ] Test against fixture package

**Depends:** ACS-005, [160](160-agent-runtime-observability.md)  
**Mission impact:** Observe intent without green gen  
**Touch:** agent_runtime_tools, codegen route helpers shared

---

### ACS-012: Multi-package smoke when sibling is red — Todo · P2

**As a** dual-loop user with product + platform  
**I want** smoke to check the package I edited  
**So that** a broken platform sibling doesn’t block product edits forever

**Acceptance criteria:**

- [ ] Document current behavior (`cargo check -p <stem>` when possible)
- [ ] Multi-package: smoke checks primary crate(s) of changed package; optional full `veil_bin` check behind flag
- [ ] Agent message when full harness check skipped due to known sibling failure
- [ ] Align with ACS-003 green fixture (sibling should be green in CI)

**Depends:** ACS-003, existing package-scoped smoke  
**Mission impact:** Product iteration unblocked  
**Touch:** `devloop.rs` smoke paths, AGENT.md

---

## Delivery order (PR stack)

| PR | Stories | Outcome |
|----|---------|---------|
| **PR1** | ACS-001, ACS-002 | Contract + Tier-0 (agents stop unwrap thrash) |
| **PR2** | ACS-003 | Green multi-package fixture + CI |
| **PR3** | ACS-004, ACS-005 | Restart + @route on stock handlers |
| **PR4** | ACS-006, ACS-007 | Ladder L0–L3 + pipeline rule |
| **PR5** | ACS-008, ACS-009 | Structured diags + palace contracts |
| **PR6** | ACS-010–012 | Bang evolution, IR routes, multi smoke polish |

**Suggested first slice:** PR1 + PR2.

---

## Success criteria (epic DoD)

An agent can, without human log-paste:

1. Follow bang contract (no `.unwrap()` after `find!`).
2. Clone ACS-003 fixture and extend a handler; CI still green.
3. After edit: smoke → (restart) → `list_routes` → `http_request` succeeds.
4. Climb L0→L3 by copying fixtures.
5. Sugar PRs always touch three phases + test.

---

## Status board

| ID | Story | Status | P |
|----|-------|--------|---|
| ACS-001 | Bang/Opt/Res contract doc | Done | P0 |
| ACS-002 | Tier-0 contract + closed loop | Done | P0 |
| ACS-003 | Green multi-package fixture + CI | Done | P0 |
| ACS-004 | Auto-restart after smoke | Done | P1 |
| ACS-005 | @route on stock handlers | Done | P1 |
| ACS-006 | Complexity ladder L0–L3 | Done | P1 |
| ACS-007 | Pipeline rule (3-phase sugar) | Done | P1 |
| ACS-008 | Structured check diagnostics | Done | P2 |
| ACS-009 | Mind Palace five contracts | Todo | P2 |
| ACS-010 | Bang portable semantics split | Todo | P2 |
| ACS-011 | list_routes from IR | Todo | P2 |
| ACS-012 | Multi-package scoped smoke | Todo | P2 |
