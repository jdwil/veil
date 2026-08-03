# Agent Edit Annotations

**Status:** Design (pre-implementation)  
**Story:** UX-030 — structured edit metadata for agent review  
**Depends on:** `EditOp` (veil-ir/src/edit.rs), DiffPanel (veil-viewer)

---

## 1. Purpose

When agents make edits, reviewers need to quickly assess **what changed** and
**how important** each change is. Today, edits arrive as a flat list of
`EditOp` values with no semantic context — no intent, no criticality signal,
no categorization.

Edit annotations add an optional structured envelope so that:

1. Agents can declare **why** they made a change (intent)
2. The system can classify changes by **category** and **criticality**
3. The DiffPanel can group/filter changes by importance
4. Review time concentrates on critical/high changes; cosmetic changes are
   collapsed by default

### 1.1 Non-goals

- Replacing the existing `EditOp` keying (still span-based)
- Requiring agents to provide annotations (fully optional — defaults inferred)
- Storing annotation history (ephemeral per edit batch, not persisted in `.veil`)
- Encoding domain knowledge in the engine (annotations are generic)

---

## 2. Schema

### 2.1 `EditAnnotation` (per-op metadata)

```rust
/// Optional metadata attached to a single EditOp by the requesting agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EditAnnotation {
    /// Short declarative description of what the edit accomplishes.
    /// Example: "Add email validation guard", "Rename for clarity".
    /// Max 120 chars (truncated by server if longer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,

    /// Structural category of the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<EditCategory>,

    /// Review priority. When omitted, inferred from lenses/shape (§3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criticality: Option<Criticality>,
}
```

### 2.2 `EditCategory` (enum)

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditCategory {
    /// Topology / naming / containment changes
    Structure,
    /// Expression bodies, control flow, logic
    Behavior,
    /// Guards, validation, invariants, type constraints
    Constraint,
    /// Ports, adapters, external wiring
    Integration,
    /// Annotations, visual metadata, non-functional
    Cosmetic,
    /// Comments, prompts, layer docs
    Docs,
}
```

### 2.3 `Criticality` (enum)

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Criticality {
    /// Potential data loss, security boundary, breaking contract change
    Critical,
    /// Important business logic change requiring careful review
    High,
    /// Standard change — default for most edits
    Normal,
    /// Trivial / cosmetic / low-risk mechanical change
    Low,
}
```

---

## 3. Criticality Inference

When an agent omits `criticality`, the server infers it from available
signals. This keeps the system useful even with dumb agents that never
annotate.

| Signal | Inferred criticality |
|--------|---------------------|
| Target construct has `lens critical` (from presentation) | **critical** |
| Target construct has `lens integration` | **high** |
| Edit is `SetBody` on a Step/Flow | **high** (behavior change) |
| Edit is `DeleteConstruct` | **high** |
| Edit is `SetAnnotations` only | **low** |
| Edit is `Rename` | **normal** |
| Edit is `SetFields` / `SetMethods` | **normal** (signature) |
| Edit is `CreateConstruct` | **normal** |

Explicit agent-provided criticality always overrides inference.

---

## 4. Wire Format

### 4.1 Request: `POST /api/edit`

The `EditRequest` gains an optional parallel array of annotations:

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct EditRequest {
    pub edits: Vec<EditOp>,
    /// Optional per-edit metadata. When present, must be same length as `edits`.
    /// Entries may be `null` (no annotation for that op).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<Option<EditAnnotation>>>,
}
```

**Why parallel array, not embedded in EditOp?**

- `EditOp` is the core AST-edit primitive — it should stay pure (shape-only,
  no review metadata). Mixing concerns would force every non-agent caller
  (property panel, palette) to supply annotations.
- Parallel array is trivially ignored by callers that don't use it.
- Serde `skip_serializing_if` means the field is absent for non-annotated
  requests (backward compatible).

### 4.2 Response: `EditResponse`

The response gains resolved annotations (with inferred criticality filled in):

```rust
#[derive(Debug, Serialize)]
pub struct EditResponse {
    pub source: String,
    pub ir: serde_json::Value,
    pub generated: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<veil_ir::Diagnostic>>,
    /// Resolved annotations (inference applied). Same length as input edits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_annotations: Option<Vec<EditAnnotation>>,
}
```

### 4.3 Diff: `GET /api/diff`

`DiffItem` gains optional annotation fields (populated when the diff
correlates to an annotated edit in the current session):

```rust
pub enum DiffItem {
    Added {
        path: String,
        node_kind: String,
        name: String,
        subkind: Option<String>,
        // NEW — populated from last edit session annotations
        annotation: Option<EditAnnotation>,
    },
    // ... same for Removed, Renamed, SignatureChanged, BodyChanged, AnnotationsChanged
}
```

**Session correlation:** The server keeps a transient map of
`(construct_path, edit_kind) → EditAnnotation` from the most recent
`POST /api/edit` batch. When `GET /api/diff` runs, it matches diff items to
this map by path + change type. Stale after file reload from disk / git.

---

## 5. TypeScript Types (store.ts)

```typescript
export interface EditAnnotation {
  intent?: string;
  category?: 'structure' | 'behavior' | 'constraint' | 'integration' | 'cosmetic' | 'docs';
  criticality?: 'critical' | 'high' | 'normal' | 'low';
}

// Updated EditRequest shape
export interface EditBatch {
  edits: EditOp[];
  annotations?: (EditAnnotation | null)[];
}
```

The existing `saveEdits(edits)` function gains an optional second parameter:

```typescript
export async function saveEdits(
  edits: EditOp[],
  annotations?: (EditAnnotation | null)[]
): Promise<boolean>;
```

---

## 6. DiffPanel Changes

### 6.1 Grouping by criticality

When annotations are present on diff items, the DiffPanel offers a grouped
view (default for agent-edited sessions):

```
▾ Critical (1)
    + guard validateEmail in CreateCustomer

▾ High (2)
    body CreateCustomer (3→5 lines)
    + Port EmailGateway

▸ Normal (4) ──────── collapsed by default when >6 items
▸ Low (2) ─────────── collapsed by default
```

### 6.2 Criticality badges

Each diff item shows a small colored dot/tag:

| Criticality | Color | Badge |
|-------------|-------|-------|
| critical | red | `!!` |
| high | orange | `!` |
| normal | — (no badge) | |
| low | dim | `·` |

### 6.3 Intent tooltip

Hovering a diff item with `intent` shows the agent's stated purpose.

---

## 7. Invariant Compliance

| Check | Answer |
|-------|--------|
| Engine encodes domain knowledge? | **No** — categories/criticality are generic (not "Aggregate-aware") |
| Inference uses layer-declared lenses? | **Yes** — `lens critical` / `lens integration` from presentation model |
| Annotations required? | **No** — fully optional; viewer still works without them |
| EditOp shape changes? | **No** — parallel array keeps EditOp pure |
| Breaks existing callers? | **No** — `annotations` field is `skip_serializing_if` absent |

---

## 8. Implementation Plan

### Phase 1: Types + inference (backend)
1. Add `EditAnnotation`, `EditCategory`, `Criticality` to `veil-ir/src/edit.rs`
2. Add `annotations` field to `EditRequest` in `protocol.rs`
3. Implement criticality inference in server (reads presentation model lenses)
4. Return `resolved_annotations` in `EditResponse`

### Phase 2: Diff correlation (backend)
5. Add transient annotation cache to server state
6. Populate `annotation` on `DiffItem` variants in `struct_diff.rs`

### Phase 3: Viewer (frontend)
7. Update `store.ts` types and `saveEdits` signature
8. Update `DiffPanel.svelte` — grouped view, criticality badges, intent tooltip
9. Agent rail: pass annotations from agent tool responses to `saveEdits`

### Phase 4: Agent integration
10. Update `write_source` / structured edit Rig tools to accept annotation hints
11. Document annotation schema in AGENT.md

---

## 9. Open Questions

| Question | Proposed Answer |
|----------|----------------|
| Persist annotations in `.veil`? | **No** — ephemeral review metadata only |
| Annotation history across edits? | **No** — latest batch only; git diff is the long-term record |
| Category inference (not just criticality)? | Defer — harder to infer reliably; let agents declare |
| Batch-level summary annotation? | Defer — per-op is sufficient for MVP |
