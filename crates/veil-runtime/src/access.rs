//! Claim-based access control — a pure, provider-agnostic predicate evaluator.
//!
//! This is a **VEIL engine primitive**. It knows nothing about Cognito, Google,
//! "employer", tenants, or any specific application. It evaluates a small,
//! composable predicate grammar against an arbitrary set of claims (a JSON
//! object). The runtime supplies the claims (extracted from a validated JWT);
//! this module only answers the question "do these claims satisfy this rule?".
//!
//! ## Rule model
//!
//! A contribution's `access` field is an [`AccessRule`]:
//!
//! - **Public** (`{ "public": true }`, or absent) → visible to everyone.
//! - **Predicate** → a leaf test over one claim (`claim`/`op`/`value`).
//! - **Combinators** → `{ "all": [..] }`, `{ "any": [..] }`, `{ "not": {..} }`.
//!
//! ```json
//! { "claim": "employer", "op": "equals", "value": "dashlx" }
//! { "claim": "cognito:groups", "op": "contains", "value": "admins" }
//! { "claim": "email", "op": "endswith", "value": "@dashlx.com" }
//! { "all": [ {"claim":"a","op":"equals","value":"1"}, {"not": {"claim":"b","op":"equals","value":"2"}} ] }
//! ```
//!
//! ## Ops
//!
//! `equals`, `not_equals`, `contains` (array membership OR substring),
//! `endswith`, `startswith`, `in` (value is a list; claim must equal one of them).
//!
//! The evaluator is total: an unsatisfiable or malformed reference (missing
//! claim, type mismatch) evaluates to `false` rather than erroring — a missing
//! claim simply means the user does not satisfy the predicate.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// A set of claims extracted from a validated token, keyed by claim name.
pub type Claims = HashMap<String, Value>;

/// An access rule attached to a contribution.
///
/// Serialization is untagged so the registration API can accept any of the
/// natural JSON shapes without a discriminant field:
/// `{"public": true}`, `{"claim":..,"op":..,"value":..}`, `{"all":[..]}`,
/// `{"any":[..]}`, `{"not":{..}}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AccessRule {
    /// Explicit public marker: `{ "public": true }` (or `false` to hide from all).
    Public { public: bool },
    /// Logical AND over sub-rules. Empty list is vacuously true.
    All {
        #[serde(rename = "all")]
        all: Vec<AccessRule>,
    },
    /// Logical OR over sub-rules. Empty list is vacuously false.
    Any {
        #[serde(rename = "any")]
        any: Vec<AccessRule>,
    },
    /// Logical negation.
    Not {
        #[serde(rename = "not")]
        not: Box<AccessRule>,
    },
    /// A leaf predicate over a single claim.
    Predicate(Predicate),
}

/// A leaf predicate: `claim <op> value`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Predicate {
    /// The claim name to read from the user's claims (e.g. "email").
    pub claim: String,
    /// The comparison operator.
    pub op: Op,
    /// The value to compare against. For `in`, this is a JSON array.
    pub value: Value,
}

/// Supported comparison operators.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Claim equals value (scalar equality).
    Equals,
    /// Claim does not equal value.
    NotEquals,
    /// If the claim is an array: membership. If it's a string: substring.
    Contains,
    /// Claim (string) ends with value (string).
    #[serde(rename = "endswith")]
    EndsWith,
    /// Claim (string) starts with value (string).
    #[serde(rename = "startswith")]
    StartsWith,
    /// Value is an array; claim must equal one of its elements.
    In,
}

impl AccessRule {
    /// Evaluate this rule against a set of claims.
    ///
    /// Returns `true` if the claims satisfy the rule. Missing claims or type
    /// mismatches produce `false` (never an error) — the semantics are
    /// "permit only when the claims demonstrably satisfy the rule".
    pub fn evaluate(&self, claims: &Claims) -> bool {
        match self {
            AccessRule::Public { public } => *public,
            AccessRule::All { all } => all.iter().all(|r| r.evaluate(claims)),
            AccessRule::Any { any } => any.iter().any(|r| r.evaluate(claims)),
            AccessRule::Not { not } => !not.evaluate(claims),
            AccessRule::Predicate(p) => p.evaluate(claims),
        }
    }

    /// Whether this rule is unconditionally public (visible to everyone,
    /// including unauthenticated callers when the app allows it).
    pub fn is_public(&self) -> bool {
        matches!(self, AccessRule::Public { public: true })
    }
}

impl Predicate {
    fn evaluate(&self, claims: &Claims) -> bool {
        let Some(claim_val) = claims.get(&self.claim) else {
            // A missing claim never satisfies a positive predicate.
            return false;
        };
        match self.op {
            Op::Equals => json_eq(claim_val, &self.value),
            Op::NotEquals => !json_eq(claim_val, &self.value),
            Op::Contains => contains(claim_val, &self.value),
            Op::EndsWith => {
                match (claim_val.as_str(), self.value.as_str()) {
                    (Some(hay), Some(suffix)) => hay.ends_with(suffix),
                    _ => false,
                }
            }
            Op::StartsWith => {
                match (claim_val.as_str(), self.value.as_str()) {
                    (Some(hay), Some(prefix)) => hay.starts_with(prefix),
                    _ => false,
                }
            }
            Op::In => match self.value.as_array() {
                Some(items) => items.iter().any(|item| json_eq(claim_val, item)),
                None => false,
            },
        }
    }
}

/// Scalar-ish JSON equality that treats numbers/strings/bools sensibly and
/// also matches a scalar against a single-element interpretation where useful.
/// Falls back to structural equality for arrays/objects.
fn json_eq(a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    // Cross-type leniency: compare string representations for scalar values so
    // that a numeric claim `1` matches a config value `"1"` (JWT claims and
    // hand-authored rules often disagree on type).
    match (scalar_as_string(a), scalar_as_string(b)) {
        (Some(sa), Some(sb)) => sa == sb,
        _ => false,
    }
}

/// `contains`: if the claim is an array, test membership; if it's a string,
/// test substring. Anything else is `false`.
fn contains(claim: &Value, needle: &Value) -> bool {
    match claim {
        Value::Array(items) => items.iter().any(|item| json_eq(item, needle)),
        Value::String(s) => match needle.as_str() {
            Some(sub) => s.contains(sub),
            None => false,
        },
        _ => false,
    }
}

/// Render a scalar JSON value as a string for lenient comparison. Returns
/// `None` for arrays/objects/null (which should compare structurally only).
fn scalar_as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims(pairs: &[(&str, Value)]) -> Claims {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn rule(j: Value) -> AccessRule {
        serde_json::from_value(j).expect("valid rule")
    }

    // ── Public ──────────────────────────────────────────────────────────────

    #[test]
    fn public_true_visible_to_all() {
        let r = rule(json!({ "public": true }));
        assert!(r.is_public());
        assert!(r.evaluate(&claims(&[])));
    }

    #[test]
    fn public_false_hidden_from_all() {
        let r = rule(json!({ "public": false }));
        assert!(!r.is_public());
        assert!(!r.evaluate(&claims(&[("email", json!("a@b.com"))])));
    }

    // ── equals / not_equals ───────────────────────────────────────────────────

    #[test]
    fn equals_matches() {
        let r = rule(json!({ "claim": "employer", "op": "equals", "value": "dashlx" }));
        assert!(r.evaluate(&claims(&[("employer", json!("dashlx"))])));
        assert!(!r.evaluate(&claims(&[("employer", json!("other"))])));
        // missing claim → false
        assert!(!r.evaluate(&claims(&[])));
    }

    #[test]
    fn equals_lenient_number_string() {
        let r = rule(json!({ "claim": "level", "op": "equals", "value": "5" }));
        assert!(r.evaluate(&claims(&[("level", json!(5))])));
    }

    #[test]
    fn not_equals() {
        let r = rule(json!({ "claim": "env", "op": "not_equals", "value": "prod" }));
        assert!(r.evaluate(&claims(&[("env", json!("dev"))])));
        assert!(!r.evaluate(&claims(&[("env", json!("prod"))])));
        // missing claim → predicate false → but not_equals of a missing claim
        // is still false (we require the claim to be present and differ).
        assert!(!r.evaluate(&claims(&[])));
    }

    // ── contains (array membership + substring) ───────────────────────────────

    #[test]
    fn contains_array_membership() {
        let r = rule(json!({ "claim": "cognito:groups", "op": "contains", "value": "admins" }));
        assert!(r.evaluate(&claims(&[("cognito:groups", json!(["users", "admins"]))])));
        assert!(!r.evaluate(&claims(&[("cognito:groups", json!(["users"]))])));
    }

    #[test]
    fn contains_substring() {
        let r = rule(json!({ "claim": "email", "op": "contains", "value": "@dashlx" }));
        assert!(r.evaluate(&claims(&[("email", json!("jd@dashlx.com"))])));
        assert!(!r.evaluate(&claims(&[("email", json!("jd@other.com"))])));
    }

    // ── endswith / startswith ────────────────────────────────────────────────

    #[test]
    fn endswith_email_domain() {
        let r = rule(json!({ "claim": "email", "op": "endswith", "value": "@dashlx.com" }));
        assert!(r.evaluate(&claims(&[("email", json!("jd@dashlx.com"))])));
        assert!(!r.evaluate(&claims(&[("email", json!("jd@nobody.example"))])));
    }

    #[test]
    fn startswith() {
        let r = rule(json!({ "claim": "sub", "op": "startswith", "value": "google_" }));
        assert!(r.evaluate(&claims(&[("sub", json!("google_12345"))])));
        assert!(!r.evaluate(&claims(&[("sub", json!("cognito_12345"))])));
    }

    // ── in ─────────────────────────────────────────────────────────────────

    #[test]
    fn in_list() {
        let r = rule(json!({ "claim": "region", "op": "in", "value": ["us-west-2", "us-east-1"] }));
        assert!(r.evaluate(&claims(&[("region", json!("us-west-2"))])));
        assert!(!r.evaluate(&claims(&[("region", json!("eu-west-1"))])));
    }

    // ── combinators ──────────────────────────────────────────────────────────

    #[test]
    fn all_and() {
        let r = rule(json!({ "all": [
            { "claim": "employer", "op": "equals", "value": "dashlx" },
            { "claim": "email", "op": "endswith", "value": "@dashlx.com" }
        ]}));
        assert!(r.evaluate(&claims(&[
            ("employer", json!("dashlx")),
            ("email", json!("jd@dashlx.com")),
        ])));
        assert!(!r.evaluate(&claims(&[
            ("employer", json!("dashlx")),
            ("email", json!("jd@other.com")),
        ])));
    }

    #[test]
    fn any_or() {
        let r = rule(json!({ "any": [
            { "claim": "role", "op": "equals", "value": "admin" },
            { "claim": "role", "op": "equals", "value": "owner" }
        ]}));
        assert!(r.evaluate(&claims(&[("role", json!("owner"))])));
        assert!(!r.evaluate(&claims(&[("role", json!("viewer"))])));
    }

    #[test]
    fn not_negation() {
        let r = rule(json!({ "not": { "claim": "banned", "op": "equals", "value": true } }));
        assert!(r.evaluate(&claims(&[("banned", json!(false))])));
        assert!(!r.evaluate(&claims(&[("banned", json!(true))])));
        // missing claim: inner predicate false → not → true
        assert!(r.evaluate(&claims(&[])));
    }

    #[test]
    fn nested_combinators() {
        // (employer == dashlx) AND NOT(email endswith @contractor.dashlx.com)
        let r = rule(json!({ "all": [
            { "claim": "employer", "op": "equals", "value": "dashlx" },
            { "not": { "claim": "email", "op": "endswith", "value": "@contractor.dashlx.com" } }
        ]}));
        assert!(r.evaluate(&claims(&[
            ("employer", json!("dashlx")),
            ("email", json!("jd@dashlx.com")),
        ])));
        assert!(!r.evaluate(&claims(&[
            ("employer", json!("dashlx")),
            ("email", json!("temp@contractor.dashlx.com")),
        ])));
    }

    #[test]
    fn empty_all_is_true_empty_any_is_false() {
        assert!(rule(json!({ "all": [] })).evaluate(&claims(&[])));
        assert!(!rule(json!({ "any": [] })).evaluate(&claims(&[])));
    }

    // ── round-trip serialization ──────────────────────────────────────────────

    #[test]
    fn round_trip_predicate() {
        let r = rule(json!({ "claim": "email", "op": "endswith", "value": "@x.com" }));
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back, json!({ "claim": "email", "op": "endswith", "value": "@x.com" }));
    }

    #[test]
    fn round_trip_public() {
        let r = rule(json!({ "public": true }));
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back, json!({ "public": true }));
    }
}
