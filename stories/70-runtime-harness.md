# Runtime Harness Stories

## Product decision (locked)

**Harnesses are VEIL-authored when possible.** Coders (and agents) should be
able to generate their own composition root via `@main` / explicit entry fns
and reusable infrastructure packages — not depend forever on handwritten
Rust bootstrap.

Handwritten `runtime/bootstrap` is **stage-0 seed only** (chicken-and-egg for
self-hosting). It shrinks as VEIL can emit a real binary that:

1. Constructs a Bus (or other integration surface)
2. Wires deps / registers handlers
3. Serves HTTP (or whatever the app needs)

`manifest.json` remains the **compiler → deployer** contract for *generic*
hosts and cloud deploys. A VEIL-authored `@main` may *consume* manifests or
wire explicitly — both are valid. Prefer explicit VEIL wiring for app-owned
harnesses; prefer manifest-driven hosts when one binary runs many packages.

### What already works

| Capability | Status |
|------------|--------|
| `@main` on fns (`di.layer`) | Yes — contributes to composed `src/main.rs` |
| Multi-`@main` section composition | Yes — `compose_main_section` by priority |
| Example composition root | `examples/di_example.veil` (`bootstrap` `@main`) |
| Custom binary without any `@main` | Partial — you can write a `fn` body, but a true
  hand-authored `fn main` package binary path needs verification/hardening |
| Multi-crate workspace + correct binary crate layout | **Gap** — main is emitted as
  workspace-level `src/main.rs`, not always a proper bin member |
| InProcessBus + axum server as VEIL-reusable package | **Gap** — lives in handwritten bootstrap / aspirational Exec |
| `provided_by: "runtime"` injection without a host | **Gap** — needs either host or VEIL-provided impls |

---

## RT-000: Prove and document VEIL-authored harness path

**Status:** Open · **Priority:** P1  
**As a** coder  
**I want** a documented, working example of a full app harness in VEIL  
**So that** I do not need handwritten Rust to run my package locally

**Acceptance criteria:**

- Example package (or extend `di_example` / hello_world) with `@main` that:
  - constructs deps
  - optionally starts a minimal HTTP or prints a CLI result
  - builds and runs via `veil gen` + `cargo run`
- Document in `docs/` or `runtime/README.md`: “Authoring your own harness”
- List known gaps (Bus impl source, multi-context registration)
- If a required primitive is missing, open/implement RT-001b rather than
  reintroducing permanent handwritten app harnesses

**Mission impact:** Validates the user’s assumption; drives gap closure.

---

## RT-001: Harness primitives as VEIL (or layer `declare`) — not eternal bootstrap

**Status:** Open · **Priority:** P1  
**As an** app author  
**I want** Bus + HTTP server + handler registration expressible in VEIL layers  
**So that** `@main` can wire a real service without Rust glue

**Acceptance criteria:**

- `InProcessBus` (or equivalent) implementable via layer `declare` + adapters,
  or a small `harness` layer / package generated into `veil_shared` / bin crate
- Pattern to register handlers from app ports/services (explicit list in VEIL
  **or** manifest reflection helper callable from VEIL)
- Stage-0 bootstrap either calls generated main or is reduced to
  `include!` / thin `cargo` entry that only invokes generated code
- No echo-only handlers presented as “done”

**Note:** Manifest-driven registration (old RT-001) remains valuable for
**generic multi-tenant hosts**; implement as a library used *from* VEIL
`@main` / Exec, not as the only path.

---

## RT-001b: Binary / workspace layout for entrypoints

**Status:** Open · **Priority:** P1  
**As a** codegen user  
**I want** `@main` / entry fns to land in a correct runnable crate  
**So that** multi-context workspaces `cargo run` cleanly

**Acceptance criteria:**

- Generated layout has a clear binary target (workspace bin or dedicated crate)
- Multi-module packages do not drop a misleading root `src/main.rs` that
  doesn’t build
- Document how `@main` interacts with library-only contexts
- Tests: single-crate and multi-context examples both produce runnable bins
  when `@main` present

---

## RT-002: Explicit vs host-injected deps

**Status:** Open · **Priority:** P1  
**As an** author  
**I want** two supported modes  
**So that** local app harnesses and hosted runtimes both work

| Mode | Who wires | Use case |
|------|-----------|----------|
| **App harness** | VEIL `@main` / `@pvd` constructs adapters and Bus | Local run, custom deploy |
| **Host harness** | External host reads `manifest.json`, injects `provided_by: runtime` | Shared platform, multi-app |

**Acceptance criteria:**

- App mode: no requirement on external host if all deps are constructible in VEIL
- Host mode: manifest fields remain stable (GEN-007); host library API documented
- `provided_by: "runtime"` clearly means “host must supply” — check warns if
  neither host nor local provider exists

---

## RT-003: End-to-end local run of a generated multi-context app

**Status:** Open · **Priority:** P1  
**As a** developer  
**I want** `veil gen && cargo run` (or `make run-example`) to exercise real handlers  
**So that** the machine loop includes “it runs,” not only “it compiles”

**Acceptance criteria:**

- One example with ≥1 real handler path (not echo)
- Cross-context Bus call if multi-context
- CI or documented manual script
- Replaces bootstrap echo as the default demo

---

## RT-004: InProcessBus as default local topology

**Status:** Open · **Priority:** P1  
**As a** local developer  
**I want** multi-context packages to share an in-process Bus  
**So that** monolith topology matches ARCHITECTURE without cloud

**Acceptance criteria:**

- Works with VEIL-authored harness (RT-001)
- Config/feature: `local` default
- HttpBus / queue buses are separate stories (cloud-specific)

---

## RT-005: Retire or quarantine handwritten bootstrap

**Status:** Open · **Priority:** P2  
**As a** maintainer  
**I want** bootstrap to be empty, generated, or clearly “seed only”  
**So that** we do not maintain two harness philosophies forever

**Acceptance criteria:**

- After RT-000–003: bootstrap either deleted, auto-generated from VEIL, or
  ≤ minimal trampoline
- Comments updated; no claim that only handwritten file registers handlers
  if VEIL path exists

---

## RT-006: Runtime README — harness vs IDE vs platform

**Status:** Open · **Priority:** P1  
**As a** new contributor  
**I want** clear roles for tools  
**So that** product scope is not confused

**Document:**

| Tool | Role |
|------|------|
| `veil serve` in project root | **Primary daily driver** — IDE, edit, check, agent prompt (soon) |
| Local veil-runtime (fs+sqlite) | **Optional** local build/platform environment |
| Cloud adapters | Deploy/test for a specific provider |
| App `@main` harness | How *this* app runs |

Include env vars, make targets, and “Authoring your own harness.”

---

## RT-007: HTTP surface documentation

**Status:** Open · **Priority:** P2  
**As a** client  
**I want** documented routes for whatever harness is running  
**So that** UI and agents do not guess

**Acceptance criteria:**

- Route table for app harness templates and platform daemon (when present)
- UI paths match or are marked future

---

## RT-008: Auth provider modes

**Status:** Open · **Priority:** P2  
**As a** deployer  
**I want** local allow-all vs real auth strategies  
**So that** app harness and host harness both compile

**Acceptance criteria:**

- Trait signatures match layers
- Local strategy documented (allow-all with log is OK for dev)
- Host can swap strategy via manifest / config
