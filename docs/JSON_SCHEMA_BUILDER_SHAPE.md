# JSON Schema Builder — Value Shape Specification

## Overview

Agent nodes in the workflow DSL support **structured output**: the LLM is
asked to return JSON matching a shape the author defines. A non-programmer
authors that shape in the IDE with a visual builder (editor id
`json_schema_builder`) — field rows, each with a name, a type dropdown, an
optional flag, and (for objects/arrays) nesting.

The builder emits a **runtime JSON value** stored on the Agent node's
`output_schema` field (`output_schema: Json`). It is deliberately **not** a
VEIL type and **not** full JSON Schema — it is a small, bounded, buildable
subset.

This document pins the exact JSON shape so two independent pieces agree:

1. **The editor** (`json_schema_builder`, builder-UI wave) — *produces* this shape.
2. **The bridge** (`veil_ir::schema_to_ty`, `crates/veil-ir/src/schema_bridge.rs`)
   — *consumes* this shape and maps it to a checkable VEIL `Ty` so that
   `agentNode.output.<field>` type-checks in downstream nodes.

If you change this shape, change **both** sides and the tests in
`schema_bridge.rs`.

## The shape

The top-level value is an object with a single `fields` key holding an ordered
array of field entries:

```json
{
  "fields": [
    { "name": "score",  "type": "number"  },
    { "name": "label",  "type": "string"  },
    { "name": "ok",     "type": "boolean" },
    { "name": "count",  "type": "integer" },
    { "name": "tags",   "type": "array",  "items": "string" },
    { "name": "note",   "type": "string", "optional": true },
    { "name": "meta",   "type": "object", "fields": [
        { "name": "source", "type": "string" },
        { "name": "weight", "type": "number" }
    ] }
  ]
}
```

### Field entry

Each entry in `fields` is an object:

| Key        | Required | Type            | Meaning |
|------------|----------|-----------------|---------|
| `name`     | yes      | string          | Field name. Must be non-empty. Unnamed/empty-named entries are dropped (they can't be referenced). |
| `type`     | yes      | string (token)  | One of the type tokens below. Missing/unknown token → the field's type is `Unknown` (tolerated). |
| `optional` | no       | boolean         | When `true`, the field type is wrapped in `Opt<…>`. Default `false`. |
| `items`    | for `array` | string token **or** field-less object schema | The array element type. |
| `fields`   | for `object` | array of field entries | The nested record's fields (recursive). |

### Type tokens

| Token       | VEIL `Ty`               | Notes |
|-------------|-------------------------|-------|
| `"string"`  | `Str`                   | |
| `"number"`  | `F64`                   | Floating point. VEIL canonical name is `F64`. |
| `"integer"` | `Int`                   | Whole number. |
| `"boolean"` | `Bool`                  | |
| `"object"`  | record `{ … }`          | Recurses on the entry's own `fields`. No `fields` ⇒ empty record. |
| `"array"`   | `List<items>`           | Recurses on `items`. No `items` ⇒ `List<Unknown>`. |

### Arrays

`items` may be either a bare token string or a nested object schema:

```json
{ "name": "tags",  "type": "array", "items": "string" }
```

```json
{ "name": "rows", "type": "array", "items": {
    "type": "object",
    "fields": [ { "name": "k", "type": "string" } ]
} }
```

→ `tags: List<Str>` and `rows: List<{ k: Str }>`.

### Nested objects

An `object` field carries its own `fields` array and becomes a nested record:

```json
{ "name": "meta", "type": "object", "fields": [
    { "name": "source", "type": "string" }
] }
```

→ `meta: { source: Str }`, and `agentNode.output.meta.source` resolves to `Str`.

## Semantics (strictness policy)

The bridge follows the **warn-for-Unknown** policy
(palace `phase4-workflow-dsl-builder-design`):

- An unrepresentable or unknown type token → `Ty::Unknown`. The typechecker
  **warns**, it does not hard-error. This avoids false positives on genuinely
  dynamic JSON.
- A field-name typo against a **known** record (e.g. `agentNode.output.scoer`
  when the field is `score`) resolves to `Unknown` at the bridge level; the
  **B2 scope resolver** is what turns that into a hard error, because the
  record is known and the field definitively does not exist.
- Malformed input never panics: it degrades to `Unknown`, or to an **empty
  record** (`{}`) for an object with no usable fields.

## Edge cases (defined behaviour)

| Input | Result |
|-------|--------|
| `{ "fields": [] }` | empty record `{}` |
| `{}` (no `fields` key) | empty record `{}` |
| field with no `name` | dropped |
| field with empty `name` | dropped |
| field with no `type` | field is `Unknown` |
| `type` an unknown token (`"decimal128"`) | field is `Unknown` |
| `array` with no `items` | `List<Unknown>` |
| top-level not an object/token (number, null, array) | `Unknown` |
| `optional: true` on an `array` | `Opt<List<…>>` |

## Field ordering

The bridge stores record fields in a `BTreeMap` (sorted by name) for
deterministic display and equality. **Authoring order is not preserved in the
`Ty`.** This is intentional — record identity is order-independent, and it
keeps tests and diagnostics stable. If the editor needs to preserve visual
order, it must do so in the authored JSON `fields` array (which is an ordered
list), not rely on the derived `Ty`.

## Consumers

- `veil_ir::schema_to_ty(&serde_json::Value) -> veil_ir::Ty`
  (`crates/veil-ir/src/schema_bridge.rs`) — the bridge. Pure, no I/O.
- **B2 scope resolver** — calls `schema_to_ty` to type `agentNode.output` as a
  record, then resolves `.field` access and flags typos.
- **Agent structured-output executor** (separate, later wave) — issues the
  Bedrock JSON-mode request matching this schema. B1 does **not** cover the
  executor; it only makes the schema type-checkable.

## Verification

`cargo test -p veil-ir -- schema_bridge` exercises every row of the tables
above. The canonical sample:

```json
{ "fields": [
    { "name": "score", "type": "number" },
    { "name": "label", "type": "string" },
    { "name": "tags",  "type": "array", "items": "string" }
] }
```

yields a record `{ label: Str, score: F64, tags: List<Str> }`.
