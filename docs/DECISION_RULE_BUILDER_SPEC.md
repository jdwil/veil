# Decision Node Rule Builder — Specification

## Overview

The Decision node in the flow composer currently has a plain text input for its `condition` field. This needs to be replaced with a visual boolean expression builder that allows constructing arbitrarily complex logic, similar to the Simple Signal builder in the wear-test app's RuleEnginePanel.

The rule builder component must be **reusable** — both the Signal builder (in the wear-test product app) and the Decision node (in the flow composer IDE) should use the same underlying component.

## Goals

1. Extract the existing rule-builder logic from `wear-test/ui.veil` into a shared, reusable component in the `application` UI layer
2. Make the component generic: it accepts available LHS fields/variables as a prop
3. Wire it into the flow composer IDE's PropertyEditor for the Decision node
4. Resolve "variables in scope" from the function signature (struct members of input params) + upstream node bindings

## Architecture

### Component Location

The reusable rule builder component should live in:
```
~/dev/veil-projects/application/ui.veil
```

If it's not already there as a standalone component, extract it from the wear-test's RuleEnginePanel and place it in the application UI layer so both consumers can import it.

### Data Model

The rule builder uses a groups/predicates model:

```
Groups[] — joined by OR
  └── Predicates[] — joined by AND within each group
        └── { path: string, op: string, value: any }
```

- `path` — dot-path to a variable/field (LHS). E.g. `event.activity_type`, `event.distance`, `result.status`
- `op` — comparison operator
- `value` — literal value or variable reference (RHS)

The serialized form is JSON: `[{ predicates: [{ path, op, value }] }]`

This gets compiled to a boolean expression tree:
```
OR(
  AND(pred1, pred2, ...),  // group 1
  AND(pred3, pred4, ...),  // group 2
)
```

### Operators

Include everything a programmer needs:

| Operator | Label | Applicable Types |
|----------|-------|-----------------|
| `Eq` | equals | all |
| `Neq` | not equals | all |
| `Gt` | greater than | numbers, dates |
| `Gte` | greater than or equal | numbers, dates |
| `Lt` | less than | numbers, dates |
| `Lte` | less than or equal | numbers, dates |
| `Contains` | contains | strings, lists |
| `NotContains` | does not contain | strings, lists |
| `StartsWith` | starts with | strings |
| `EndsWith` | ends with | strings |
| `In` | is in | value in list |
| `NotIn` | is not in | value not in list |
| `Exists` | exists (not null) | all |
| `NotExists` | does not exist (is null) | all |
| `Matches` | matches regex | strings |
| `Between` | between (inclusive) | numbers, dates |

### LHS Variables (Fields in Scope)

The rule builder receives its available LHS fields as a prop: `fields: { path: string, type: string, label?: string }[]`

For the **Decision node in the flow composer**, these fields come from:

1. **Function signature parameters** — the `fn run(ctx: Json)` or `fn handle(event: MyEvent)` input params. If the param is a struct type, expand its members as dot-paths:
   - `event.activity_type` (Str)
   - `event.distance` (F64)
   - `event.timestamp` (Dt)

2. **Upstream node bindings** — variables created by nodes that execute before this Decision node in the graph:
   - `result_binding` from RepositoryAccess/Query/Relay nodes upstream
   - `binding` from Assign nodes upstream
   - `item_binding` / `index_binding` from enclosing Loop nodes

For the **Signal builder in wear-test**, the fields come from the event type's schema (already working — the event fields are hardcoded or fetched from the backend).

### Component Interface

```typescript
interface RuleBuilderProps {
  /** Available LHS fields with types */
  fields: { path: string; type: string; label?: string }[];
  /** Current value as JSON string of groups/predicates */
  value: string;
  /** Callback when the value changes */
  onChange: (json: string) => void;
  /** Optional: compact mode for inline embedding */
  compact?: boolean;
}
```

### UI Layout

```
┌─────────────────────────────────────────────────┐
│ Condition                                        │
├─────────────────────────────────────────────────┤
│ ┌─── Group 1 (AND) ──────────────────────────┐  │
│ │ [field ▼] [operator ▼] [value input]   [✕] │  │
│ │ [field ▼] [operator ▼] [value input]   [✕] │  │
│ │                          [+ Add condition]  │  │
│ └─────────────────────────────────────────────┘  │
│                      — OR —                      │
│ ┌─── Group 2 (AND) ──────────────────────────┐  │
│ │ [field ▼] [operator ▼] [value input]   [✕] │  │
│ │                          [+ Add condition]  │  │
│ └─────────────────────────────────────────────┘  │
│                                [+ Add OR group]  │
└─────────────────────────────────────────────────┘
```

## Implementation Steps

### Step 1: Extract Rule Builder Component

1. Read the current rule builder logic in `~/dev/veil-projects/application/ui.veil` (the wear-test's RuleEnginePanel section)
2. Identify the UI elements: group container, predicate row, field select, operator select, value input, add/remove buttons
3. Extract into a standalone component declaration in `application/ui.veil` with the `RuleBuilderProps` interface above
4. The component should be self-contained: groups JSON in, groups JSON out via onChange callback

### Step 2: Generalize the LHS Fields

Currently the Signal builder hardcodes event fields or derives them from `sig_event`. Change this to accept a `fields` prop so the component is event-agnostic.

The field select dropdown should:
- Show `label` (or `path` if no label) as the display text
- Use `path` as the value
- Optionally show `type` as a hint (e.g., greyed suffix)

The operator select should filter operators based on the selected field's `type`:
- String fields: all string operators
- Number fields: numeric comparison operators
- Boolean fields: Eq, Neq, Exists, NotExists
- Date fields: comparison + Between
- Any/unknown: all operators

### Step 3: Wire into the Decision Node (Flow Composer IDE)

In `veil-viewer/src/lib/PropertyEditor.svelte`:

1. When rendering a Decision node's `condition` field, instead of a plain text input, render the RuleBuilder component
2. The component needs to be available in the viewer — since it's a VEIL-generated component in the application layer, it'll need to be either:
   - **Option A**: Embedded as a Svelte component directly in the viewer (imported from a shared package)
   - **Option B**: Rendered via an iframe/web component pointing at the application frontend
   - **Option C**: Re-implemented as a native Svelte component in the viewer that mirrors the VEIL component's behavior

   **Recommended: Option C** — implement a native `RuleBuilder.svelte` in `veil-viewer/src/lib/editors/` that shares the same data model. The VEIL-generated component in the application layer uses the same JSON format, so they're interoperable even if the implementations are separate. This avoids cross-origin/iframe complexity.

3. The `condition` property on the Decision node changes from a plain string to a JSON string containing the groups/predicates model.

### Step 4: Resolve Variables in Scope

Create a function in the viewer that computes available variables for a given node position in the graph:

```typescript
function resolveFieldsInScope(graph: IrGraph, nodeId: number): { path: string; type: string; label?: string }[] {
  // 1. Find the parent Flow/fn node
  // 2. Get its input params from properties (params: "(ctx: Json)" or "(event: MyEvent)")
  // 3. If param type is a struct defined in the IR, expand its fields as dot-paths
  // 4. Walk upstream nodes (predecessors via edges) and collect bindings:
  //    - result_binding from Query/RepositoryAccess/Relay nodes
  //    - binding from Assign nodes  
  //    - item_binding from enclosing Loop
  // 5. Return combined list
}
```

This function should be called when the PropertyEditor opens for a Decision node, and the result passed as the `fields` prop to the RuleBuilder.

### Step 5: Update Signal Builder to Use Shared Component

Once the shared RuleBuilder component is working in the viewer, update the wear-test's Signal builder to also use the extracted component from `application/ui.veil`, removing the inline implementation. This ensures both UIs stay in sync.

## Layer Changes

### `flow/layers/main.layer` — Decision construct

The `condition: Str` field type stays the same (it's still a string at the storage level — just JSON-encoded now). But add a `field_hints` entry to tell the IDE to render it with the rule builder:

```
construct Decision
  ...
  has
    condition: Str
    true_label: Str
    false_label: Str
  field_hints
    condition: editor rule_builder
    true_label: label "True label"
    false_label: label "False label"
```

The `editor rule_builder` hint tells the PropertyEditor to use the RuleBuilder component instead of a plain text input.

## Testing

1. Open the wear-test app → Signals → Simple Signal → verify the rule builder still works identically
2. Open the flow composer IDE from wear-test → drag a Decision node → click it → verify the rule builder appears in the property editor with LHS fields from the function signature
3. Add multiple conditions across multiple groups → verify the JSON serializes correctly
4. Add upstream nodes (Query, Assign) before the Decision → verify their bindings appear as available LHS fields

## Files Involved

- `~/dev/veil-projects/application/ui.veil` — shared RuleBuilder component definition
- `~/dev/veil-projects/wear-test/ui.veil` — Signal builder updated to use shared component
- `~/dev/veil-projects/flow/layers/main.layer` — Decision field_hints update
- `~/dev/jd/veil/veil-viewer/src/lib/editors/RuleBuilder.svelte` — native viewer implementation
- `~/dev/jd/veil/veil-viewer/src/lib/PropertyEditor.svelte` — wire RuleBuilder for Decision nodes
- `~/dev/jd/veil/veil-viewer/src/lib/store.ts` or new `scope.ts` — resolveFieldsInScope function
