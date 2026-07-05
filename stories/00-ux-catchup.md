# UX Catch-Up Stories

These stories bring the viewer/editor UX up to speed with the current engine capabilities.

---

## UX-001: Dynamic palette from layer registry

**As a** user viewing a .veil file
**I want** the construct palette to show only constructs defined by the loaded layers
**So that** I see relevant options without hardcoded DDD assumptions

**Acceptance criteria:**
- Palette reads from `/api/palette` endpoint (already exists)
- Constructs show layer-defined icons, colors, and labels
- Statements (dispatch, guard, etc.) appear in a separate palette section
- Changing the `use` line updates the palette without restart

---

## UX-002: Declared constructs visible in viewer

**As a** user working with a layer that declares constructs (e.g. Bus trait)
**I want** declared/injected constructs to appear in the node graph
**So that** I can see the full solution structure including layer-provided infrastructure

**Acceptance criteria:**
- Declared constructs (e.g. Bus) show in the top-level graph
- They're visually distinguished (e.g. dimmed or dashed border) as layer-provided vs user-authored
- Clicking them shows their methods in the property editor

---

## UX-003: Flow body step visualization

**As a** user viewing a fn-shaped construct (service, saga, flow)
**I want** to see the step bodies with their expressions rendered
**So that** I can understand the flow logic visually

**Acceptance criteria:**
- Drilling into a fn-shaped construct shows steps as nodes
- Each step node shows its body expressions (calls, guards, assignments)
- Desugared statements (dispatch → Bus.dispatch) show with original keyword icon
- Guard statements show condition and failure message

---

## UX-004: Aggregate impl visualization

**As a** user viewing a struct-shaped construct with fn blocks (e.g. Customer aggregate)
**I want** to see the business logic methods and their bodies
**So that** I can understand invariants, state transitions, and event emissions

**Acceptance criteria:**
- Struct nodes with `fns` show a "Methods" section in property editor
- Each method shows: name, params, return type, @invariant annotation
- Body shows assignments (field mutations) and emit statements
- State machine visualization shows valid transitions

---

## UX-005: Generated code preview panel

**As a** user editing VEIL source in the viewer
**I want** to see the generated Rust code in a side panel
**So that** I can verify the codegen produces what I expect

**Acceptance criteria:**
- A "Generated" tab shows the Rust output for the selected construct
- Updates live as the user edits in the property panel
- Syntax highlighted
- Can toggle between domain/types.rs, ports/mod.rs, application/mod.rs views

---

## UX-006: Stub file visualization

**As a** user who has loaded .stub files
**I want** to see external crate APIs in the palette/graph
**So that** I can reference them when building adapters

**Acceptance criteria:**
- Stub crates appear in a dedicated palette section ("External")
- Each stub shows its structs and methods
- Hovering shows method signatures
- Can be referenced in adapter impl bodies

---

## UX-007: Orchestrator visualization with Bus routing

**As a** user viewing an orchestrator/saga
**I want** to see that cross-context calls go through the Bus
**So that** I understand the communication architecture

**Acceptance criteria:**
- Orchestrator steps with `ctx` refs show which context handles each step
- Calls are visually shown as Bus-mediated (arrow through Bus node)
- The Bus node shows its methods and which statements map to them
- Compensation blocks (if present) shown as secondary flow

---

## UX-008: Cross-module navigation

**As a** user working in a multi-context solution
**I want** to navigate between modules (contexts) easily
**So that** I can understand the full system

**Acceptance criteria:**
- Top-level view shows all modules as nodes
- Clicking a module drills into it
- Breadcrumbs show current location
- Back navigation works
- Orchestrator view shows referenced contexts as linked nodes
