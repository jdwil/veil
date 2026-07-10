# Serialize & Edit Integrity Stories

Mission: deterministic round-trips; edit→save must not corrupt agent-authored source.

---

## SER-001: Lossless field/annotation serialization

**Status:** Done · **Priority:** P0  
**As an** author (agent or human via IDE)  
**I want** field annotations and defaults preserved on emit  
**So that** saving after edit does not strip `@dep` and similar policy

**Acceptance criteria:**

- Serializer emits field/input annotations (`@dep`, `@env`, …) in canonical form
- Serializer emits `default_expr` when present
- Parse → serialize → parse preserves annotations on all `examples/*.veil`
  that use them (at least `di_example`, onboarding, runtime source)
- Golden or round-trip tests fail if annotations drop

**Touch:** `crates/veil-ir/src/serialize.rs`

**Mission impact:** DI and layer policy cannot survive the editor today.

---

## SER-002: Full expression body serialization

**Status:** Done · **Priority:** P0  
**As an** author  
**I want** control-flow and pattern expressions to re-emit completely  
**So that** round-trips do not replace bodies with `...` placeholders

**Acceptance criteria:**

- `IfExpr`, `IfLet`, `WhileLet`, `While`, `For`, `Match`, `Loop`, closures
  serialize to valid VEIL that re-parses to equivalent AST
- No `"..."` placeholder remains for supported expr kinds
- Round-trip tests on fixtures covering each kind
- Document any intentionally unsupported form

**Mission impact:** Critical bodies are exactly what humans review — must not mangle.

---

## SER-003: Canonical format + churn control

**Status:** Done · **Priority:** P1  
**As an** agent reviewing diffs  
**I want** re-serialize to change only semantically edited regions  
**So that** noise does not destroy trust

**Acceptance criteria:**

- Documented canonical formatting rules (indent, blank lines, annotation order) — `docs/SERIALIZE.md`
- `veil emit` is idempotent: second emit is a no-op diff on clean trees
- Prefer stable ordering (source order for children/fields)
- Typed assigns (`name: Type = expr`) and bare-ident sub-block detection fixed so emit does not reorder/churn
- Optional: span-preserving edit mode that rewrites only touched constructs — **deferred** (full re-emit is canonical)

**Mission impact:** Noisy pretty-print breaks agent and human trust.

**Done notes:** Canonical `pkg`/`+`/bare calls; `finish()` trailing newline; `emit_items_spaced` for layer-provided blanks; typed `Assign` type ann; `is_sub_block_header` requires Indent.

---

## SER-004: Round-trip test suite

**Status:** Done · **Priority:** P0  
**As a** maintainer  
**I want** automated parse→emit→parse (and ideally emit equality) on all examples  
**So that** serializer regressions never ship silent

**Acceptance criteria:**

- CI runs round-trip on `examples/**/*.veil`, `layers` declare samples if needed,
  and `runtime/src/*.veil` — `cargo test -p veil-parser --test roundtrip_suite` / `make test-roundtrip`
- Assert emit equality after second parse (idempotent emit)
- Assert annotation and body preservation cases from SER-001/002
- Fail the build on mismatch

**Done notes:**

- Suite: `crates/veil-parser/tests/roundtrip_suite.rs`
- Enum variant lits emit as `Enum.Variant` (not `::`) so runtime.veil is idempotent
- Layer resolution walks ancestors + `VEIL_LAYERS_DIR` (cargo-test CWD safe)
- Known unparseable (svelte page/layout StringLit): `examples/customer_portal.veil`,
  `runtime/src/runtime-ui.veil` — allowlisted until parser supports them

---

## SER-005: Edit ops complete and safe

**Status:** Open · **Priority:** P1  
**As a** viewer user  
**I want** structured edits that match server capabilities without hacks  
**So that** the IDE does not cast `as any` or store bodies as opaque Idents forever

**Acceptance criteria:**

- Client `EditOp` type matches server (`set_body`, create, delete, …)
- `SetBody` parses VEIL expression text into real `Expr` AST (not only `Ident` lines)
- Invalid body text returns a diagnostic, does not corrupt file
- Spans for newly created nodes are stable enough for subsequent edits
  (or edits key by node id exclusively — document which)

**Touch:** `veil-ir/edit.rs`, `veil-viewer/src/lib/store.ts`, server API

---

## SER-006: Delete construct / delete node op

**Status:** Open · **Priority:** P1  
**As a** human restructuring a package  
**I want** delete to persist to source  
**So that** canvas Backspace is not a lie

**Acceptance criteria:**

- `EditOp::DeleteConstruct { node_id }` (or equivalent) removes AST node and children
- Re-serialize + validate; refuse delete if constraints would break (or warn)
- Viewer Delete/Backspace calls the API
- Undo optional later; for now confirm dialog is acceptable
