//! `veil-jwks` — a generic JWKS-backed JWT validator for VEIL auth applications.
//!
//! This crate is the concrete backing for the platform `veil-jwks` VEIL stub
//! (aliased as `Jwks` in product code). It exposes exactly two composite
//! primitives that the stub maps to:
//!
//! - [`validate`] — fetch a JWKS key set from a JWKS URL, verify an RS256 JWT's
//!   signature + expiry (+ optional audience), and return the decoded claims as
//!   [`serde_json::Value`].
//! - [`decode_unverified`] — base64-decode a JWT payload **without** signature
//!   verification (dev-only convenience).
//!
//! The crate is intentionally generic: it knows nothing about Cognito pools,
//! client IDs, or issuers. Callers pass a full JWKS URL and an optional audience
//! (`client_id`). Issuer/pool/client are the *product's* `@env` config, kept out
//! of this reusable primitive.
//!
//! The implementation is lifted and generalized from
//! `veil-runtime/src/auth.rs` (`JwksKeySet::fetch_cognito` + `validate_token`),
//! where the JWKS URL was hardcoded to Cognito's
//! `{issuer}/.well-known/jwks.json`. Here the caller supplies the URL directly.

use base64::Engine as _;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;

/// Raw JWKS document as returned by an OIDC `.well-known/jwks.json` endpoint.
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKeyRaw>,
}

/// A single raw JWK entry. Only RSA keys are usable for RS256 validation.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JwkKeyRaw {
    kid: String,
    kty: String,
    alg: Option<String>,
    n: String,
    e: String,
    #[serde(rename = "use")]
    use_field: Option<String>,
}

/// A usable decoding key paired with its key id.
struct JwkKey {
    kid: String,
    decoding_key: DecodingKey,
}

/// Fetch a JWKS key set from the given URL and build RSA decoding keys.
async fn fetch_key_set(jwks_url: &str) -> Result<Vec<JwkKey>, String> {
    let resp: JwksResponse = reqwest::Client::new()
        .get(jwks_url)
        .send()
        .await
        .map_err(|e| format!("JWKS fetch failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("JWKS parse failed: {e}"))?;

    let mut keys = Vec::new();
    for raw in resp.keys {
        if raw.kty != "RSA" {
            continue;
        }
        if let Ok(dk) = DecodingKey::from_rsa_components(&raw.n, &raw.e) {
            keys.push(JwkKey {
                kid: raw.kid,
                decoding_key: dk,
            });
        }
    }

    if keys.is_empty() {
        return Err("No valid RSA keys found in JWKS".into());
    }

    Ok(keys)
}

/// Fetch JWKS from `jwks_url`, validate an RS256 JWT's signature + expiry
/// (and audience when `client_id` is `Some`), and return the decoded claims.
///
/// Security checks performed:
/// - signature against the JWKS RSA key matching the token's `kid`
/// - expiry (`exp`)
/// - audience (`aud`) only when `client_id` is `Some(non_empty)`; when `None`
///   or empty, audience validation is skipped (e.g. Cognito access tokens omit
///   `aud`).
///
/// Issuer is intentionally **not** enforced here: the JWKS URL already binds the
/// trust root to a specific provider, and issuer/pool config belongs to the
/// product, not this generic primitive.
///
/// The `client_id` and string params are taken by reference (`&str` /
/// `&Option<String>`) to line up with how VEIL codegen borrows `Str` /
/// `Opt<Str>` arguments at the call site.
///
/// Returns the full claim set as a JSON object on success, or a human-readable
/// error string on any failure.
pub async fn validate(
    jwks_url: &str,
    token: &str,
    client_id: &Option<String>,
) -> Result<Value, String> {
    let keys = fetch_key_set(jwks_url).await?;

    let header = decode_header(token).map_err(|e| format!("invalid token header: {e}"))?;
    let kid = header
        .kid
        .ok_or_else(|| "invalid token: missing kid".to_string())?;

    let key = keys
        .iter()
        .find(|k| k.kid == kid)
        .map(|k| &k.decoding_key)
        .ok_or_else(|| format!("invalid token: unknown kid: {kid}"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = true;
    match client_id.as_deref() {
        Some(aud) if !aud.is_empty() => validation.set_audience(&[aud]),
        _ => validation.validate_aud = false,
    }

    let token_data = decode::<serde_json::Map<String, Value>>(token, key, &validation)
        .map_err(|e| format!("invalid token: {e}"))?;

    Ok(Value::Object(token_data.claims))
}

/// Decode a JWT's payload **without** verifying its signature.
///
/// Dev-only. Splits the token on `.`, base64url-decodes the payload segment,
/// and parses it as JSON. Never use for authorization decisions.
pub fn decode_unverified(token: &str) -> Result<Value, String> {
    let payload_b64 = token
        .split('.')
        .nth(1)
        .ok_or_else(|| "invalid token: missing payload segment".to_string())?;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload_b64))
        .map_err(|e| format!("invalid token payload base64: {e}"))?;

    serde_json::from_slice(&bytes).map_err(|e| format!("invalid token payload json: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_unverified_reads_payload_claims() {
        // header.payload.signature — payload = {"sub":"abc","email":"a@b.c"}
        // base64url(no pad) of the payload JSON:
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"abc","email":"a@b.c"}"#);
        let token = format!("aGVhZGVy.{payload}.c2ln");
        let claims = decode_unverified(&token).expect("decode ok");
        assert_eq!(claims["sub"], "abc");
        assert_eq!(claims["email"], "a@b.c");
    }

    #[test]
    fn decode_unverified_rejects_malformed() {
        assert!(decode_unverified("only-one-segment").is_err());
        assert!(decode_unverified("h.!!!notbase64!!!.s").is_err());
    }
}
