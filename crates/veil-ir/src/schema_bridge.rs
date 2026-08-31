//! JSON-schema-builder value → checkable [`Ty`] bridge (Phase 4, blocker #2/#4).
//!
//! Agent nodes in the workflow DSL support **structured output**: a
//! non-programmer defines the JSON shape of the LLM's response via a visual
//! schema builder in the IDE (editor `json_schema_builder`). That shape is
//! authored as a *runtime JSON value* (`output_schema: Json`), **not** as a
//! VEIL type. But downstream nodes must reference `agentNode.output.<field>`
//! and have it **type-checked**.
//!
//! This module bridges that gap: [`schema_to_ty`] maps an authored schema
//! value into a VEIL [`Ty`] (a [`Ty::Record`] with typed fields) so the
//! typechecker can resolve field access against it:
//!
//! - `agentNode.output`        → the record `Ty`
//! - `agentNode.output.score`  → `F64` (enables `score > 0.5` to typecheck)
//! - `agentNode.output.scoer`  → `Unknown` (a known-record typo → error in B2)
//!
//! ## Pinned JSON shape (what `json_schema_builder` emits)
//!
//! Deliberately a **small, buildable subset** of JSON Schema — not the full
//! spec. The editor and this bridge MUST agree on exactly this shape. See
//! `docs/JSON_SCHEMA_BUILDER_SHAPE.md` for the authoritative spec.
//!
//! ```json
//! {
//!   "fields": [
//!     { "name": "score",  "type": "number" },
//!     { "name": "label",  "type": "string" },
//!     { "name": "ok",     "type": "boolean" },
//!     { "name": "count",  "type": "integer" },
//!     { "name": "tags",   "type": "array",  "items": "string" },
//!     { "name": "note",   "type": "string", "optional": true },
//!     { "name": "meta",   "type": "object", "fields": [
//!         { "name": "source", "type": "string" }
//!     ] }
//!   ]
//! }
//! ```
//!
//! ## Type mapping
//!
//! | schema `type` | VEIL `Ty`                       |
//! |---------------|---------------------------------|
//! | `"string"`    | `Named("Str")`                  |
//! | `"number"`    | `Named("F64")`                  |
//! | `"integer"`   | `Named("Int")`                  |
//! | `"boolean"`   | `Named("Bool")`                 |
//! | `"object"`    | `Record{ .. }` (recurse)        |
//! | `"array"`     | `List<items>` (recurse `items`) |
//! | `optional:true` field | wraps its `Ty` in `Opt<..>` |
//! | unknown / malformed | `Unknown`                 |
//!
//! `Unknown` is tolerated, never a hard error here — consistent with the
//! warn-strictness policy (palace `phase4-workflow-dsl-builder-design`): an
//! unrepresentable/unknown token yields `Ty::Unknown`, which the checker warns
//! on rather than rejecting. Only a *known* field-name typo on a known record
//! is an error, and that decision lives in the B2 scope resolver.
//!
//! This bridge is **pure** (no I/O, no registry) and lives in `veil-ir` so the
//! typechecker (B2) can call it without pulling `veil-codegen` inference
//! upstream.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::typecheck::Ty;

/// The top-level key holding the ordered list of fields in an authored schema.
const FIELDS_KEY: &str = "fields";
/// The key naming a field.
const NAME_KEY: &str = "name";
/// The key selecting a field's type token.
const TYPE_KEY: &str = "type";
/// The key marking a field optional (→ `Opt<T>`).
const OPTIONAL_KEY: &str = "optional";
/// The key holding an array's element type (a nested type token or object).
const ITEMS_KEY: &str = "items";

/// Bridge an Agent node's authored `output_schema` JSON value into a VEIL
/// [`Ty`] the typechecker can resolve field access against.
///
/// The top-level schema is expected to be an object with a `fields` array
/// (see the module docs for the pinned shape); it becomes a [`Ty::Record`].
///
/// Robustness / warn-strictness: any malformed or unrepresentable input
/// degrades to [`Ty::Unknown`] (or an empty record for an empty/absent field
/// list) rather than panicking or erroring — the checker warns on `Unknown`.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use veil_ir::schema_to_ty;
///
/// let schema = json!({
///     "fields": [
///         { "name": "score", "type": "number" },
///         { "name": "label", "type": "string" },
///         { "name": "tags",  "type": "array", "items": "string" }
///     ]
/// });
/// let ty = schema_to_ty(&schema);
/// // ty is a record: { label: Str, score: F64, tags: List<Str> }
/// assert!(ty.is_record());
/// ```
pub fn schema_to_ty(schema: &Json) -> Ty {
    match schema {
        // Canonical top-level: an object carrying a `fields` array.
        Json::Object(map) => object_to_ty(map),
        // A bare token string ("string", "number", …) is also accepted so the
        // function composes (e.g. array `items` reuse the same path).
        Json::String(tok) => token_to_ty(tok),
        // Anything else at the top level is not a schema we understand.
        _ => Ty::Unknown,
    }
}

/// Convert a JSON object (either the top-level schema or a nested `object`
/// field) into a [`Ty::Record`]. An object with no usable `fields` becomes an
/// **empty record** (`Record({})`) — a defined, checkable shape whose every
/// field access is `Unknown`.
fn object_to_ty(map: &serde_json::Map<String, Json>) -> Ty {
    let mut fields: BTreeMap<String, Ty> = BTreeMap::new();

    if let Some(Json::Array(list)) = map.get(FIELDS_KEY) {
        for entry in list {
            if let Some((name, ty)) = field_to_ty(entry) {
                fields.insert(name, ty);
            }
        }
    }

    Ty::Record(fields)
}

/// Convert one field entry `{ name, type, optional?, items?, fields? }` into a
/// `(name, Ty)` pair. Returns `None` when the entry has no usable name (an
/// unnamed field cannot be referenced, so it is dropped).
fn field_to_ty(entry: &Json) -> Option<(String, Ty)> {
    let obj = entry.as_object()?;
    let name = obj.get(NAME_KEY)?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }

    let mut ty = type_of_field(obj);

    // `optional: true` wraps the resolved type in Opt<..> (idempotent: never
    // double-wraps a token that already resolved to Opt).
    let optional = obj.get(OPTIONAL_KEY).and_then(Json::as_bool).unwrap_or(false);
    if optional && !matches!(ty, Ty::Opt(_)) {
        ty = Ty::Opt(Box::new(ty));
    }

    Some((name, ty))
}

/// Resolve the [`Ty`] of a field object from its `type` token and any
/// structural extras (`items` for arrays, nested `fields` for objects).
fn type_of_field(obj: &serde_json::Map<String, Json>) -> Ty {
    let token = match obj.get(TYPE_KEY).and_then(Json::as_str) {
        Some(t) => t,
        None => return Ty::Unknown,
    };

    match token {
        "object" => {
            // Nested record: recurse on this same object (it carries its own
            // `fields`). An object field with no nested fields → empty record.
            object_to_ty(obj)
        }
        "array" => {
            // `items` may be a bare token string ("string") or a nested object
            // schema ({ "type": "object", "fields": [...] }). Missing items →
            // List<Unknown>.
            let elem = match obj.get(ITEMS_KEY) {
                Some(items) => items_to_ty(items),
                None => Ty::Unknown,
            };
            Ty::List(Box::new(elem))
        }
        other => token_to_ty(other),
    }
}

/// Resolve an array's `items` value into its element [`Ty`]. Accepts a bare
/// token string or a nested object/array schema value.
fn items_to_ty(items: &Json) -> Ty {
    match items {
        Json::String(tok) => token_to_ty(tok),
        Json::Object(map) => {
            // Nested object or array element described inline.
            match map.get(TYPE_KEY).and_then(Json::as_str) {
                Some("object") => object_to_ty(map),
                Some("array") => type_of_field(map),
                Some(tok) => token_to_ty(tok),
                None => Ty::Unknown,
            }
        }
        _ => Ty::Unknown,
    }
}

/// Map a scalar schema type token to its VEIL [`Ty`]. Unknown tokens →
/// [`Ty::Unknown`] (tolerated, warned — never a hard error).
///
/// Names are VEIL-canonical (aligned with `typecheck::normalize_type_name`):
/// `number → F64`, `integer → Int`, `string → Str`, `boolean → Bool`.
fn token_to_ty(token: &str) -> Ty {
    match token {
        "string" => Ty::Named("Str".into()),
        "number" => Ty::Named("F64".into()),
        "integer" => Ty::Named("Int".into()),
        "boolean" => Ty::Named("Bool".into()),
        // `object` / `array` handled structurally by callers; a bare token of
        // either (no structure) degrades to an empty record / unknown list.
        "object" => Ty::Record(BTreeMap::new()),
        "array" => Ty::List(Box::new(Ty::Unknown)),
        _ => Ty::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(name: &str) -> Ty {
        Ty::Named(name.into())
    }

    #[test]
    fn flat_object_maps_to_record_with_field_types() {
        let schema = json!({
            "fields": [
                { "name": "score",  "type": "number"  },
                { "name": "label",  "type": "string"  },
                { "name": "ok",     "type": "boolean" },
                { "name": "count",  "type": "integer" }
            ]
        });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("score".to_string(), s("F64"));
        expected.insert("label".to_string(), s("Str"));
        expected.insert("ok".to_string(), s("Bool"));
        expected.insert("count".to_string(), s("Int"));
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn verification_sample_from_spec() {
        // Exactly the sample in the B1 spec's Verification section.
        let schema = json!({
            "fields": [
                { "name": "score", "type": "number" },
                { "name": "label", "type": "string" },
                { "name": "tags",  "type": "array", "items": "string" }
            ]
        });
        let ty = schema_to_ty(&schema);
        match &ty {
            Ty::Record(fields) => {
                assert_eq!(fields.get("score"), Some(&s("F64")));
                assert_eq!(fields.get("label"), Some(&s("Str")));
                assert_eq!(
                    fields.get("tags"),
                    Some(&Ty::List(Box::new(s("Str"))))
                );
            }
            other => panic!("expected record, got {other:?}"),
        }
    }

    #[test]
    fn nested_object_maps_to_nested_record() {
        let schema = json!({
            "fields": [
                { "name": "id", "type": "string" },
                { "name": "meta", "type": "object", "fields": [
                    { "name": "source", "type": "string" },
                    { "name": "weight", "type": "number" }
                ] }
            ]
        });
        let ty = schema_to_ty(&schema);
        let mut inner = BTreeMap::new();
        inner.insert("source".to_string(), s("Str"));
        inner.insert("weight".to_string(), s("F64"));
        let mut outer = BTreeMap::new();
        outer.insert("id".to_string(), s("Str"));
        outer.insert("meta".to_string(), Ty::Record(inner));
        assert_eq!(ty, Ty::Record(outer));
    }

    #[test]
    fn array_of_string_maps_to_list_str() {
        let schema = json!({
            "fields": [ { "name": "tags", "type": "array", "items": "string" } ]
        });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("tags".to_string(), Ty::List(Box::new(s("Str"))));
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn array_of_object_maps_to_list_of_record() {
        let schema = json!({
            "fields": [ { "name": "rows", "type": "array", "items": {
                "type": "object",
                "fields": [ { "name": "k", "type": "string" } ]
            } } ]
        });
        let ty = schema_to_ty(&schema);
        let mut row = BTreeMap::new();
        row.insert("k".to_string(), s("Str"));
        let mut expected = BTreeMap::new();
        expected.insert("rows".to_string(), Ty::List(Box::new(Ty::Record(row))));
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn optional_field_wraps_in_opt() {
        let schema = json!({
            "fields": [
                { "name": "note", "type": "string", "optional": true },
                { "name": "req",  "type": "string" }
            ]
        });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("note".to_string(), Ty::Opt(Box::new(s("Str"))));
        expected.insert("req".to_string(), s("Str"));
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn optional_array_wraps_the_list() {
        let schema = json!({
            "fields": [
                { "name": "tags", "type": "array", "items": "string", "optional": true }
            ]
        });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert(
            "tags".to_string(),
            Ty::Opt(Box::new(Ty::List(Box::new(s("Str"))))),
        );
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn unknown_type_token_yields_unknown_field() {
        let schema = json!({
            "fields": [ { "name": "weird", "type": "decimal128" } ]
        });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("weird".to_string(), Ty::Unknown);
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn field_missing_type_yields_unknown() {
        let schema = json!({ "fields": [ { "name": "mystery" } ] });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("mystery".to_string(), Ty::Unknown);
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn unnamed_field_is_dropped() {
        let schema = json!({
            "fields": [
                { "type": "string" },            // no name → dropped
                { "name": "", "type": "string" },// empty name → dropped
                { "name": "keep", "type": "string" }
            ]
        });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("keep".to_string(), s("Str"));
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn empty_schema_is_empty_record() {
        // Object with an empty fields list.
        assert_eq!(schema_to_ty(&json!({ "fields": [] })), Ty::Record(BTreeMap::new()));
        // Object with no fields key at all.
        assert_eq!(schema_to_ty(&json!({})), Ty::Record(BTreeMap::new()));
    }

    #[test]
    fn non_object_non_string_top_level_is_unknown() {
        assert_eq!(schema_to_ty(&json!(42)), Ty::Unknown);
        assert_eq!(schema_to_ty(&json!(null)), Ty::Unknown);
        assert_eq!(schema_to_ty(&json!([1, 2, 3])), Ty::Unknown);
    }

    #[test]
    fn array_missing_items_is_list_unknown() {
        let schema = json!({ "fields": [ { "name": "xs", "type": "array" } ] });
        let ty = schema_to_ty(&schema);
        let mut expected = BTreeMap::new();
        expected.insert("xs".to_string(), Ty::List(Box::new(Ty::Unknown)));
        assert_eq!(ty, Ty::Record(expected));
    }

    #[test]
    fn record_field_access_resolves_types() {
        // The whole point: a record's field resolves to the declared Ty and a
        // typo resolves to Unknown. Verified through the public Ty helpers.
        let schema = json!({
            "fields": [
                { "name": "score", "type": "number" },
                { "name": "tags",  "type": "array", "items": "string" }
            ]
        });
        let ty = schema_to_ty(&schema);
        assert_eq!(ty.record_field("score"), Some(s("F64")));
        assert_eq!(ty.record_field("tags"), Some(Ty::List(Box::new(s("Str")))));
        assert_eq!(ty.record_field("scoer"), None); // typo
    }
}
