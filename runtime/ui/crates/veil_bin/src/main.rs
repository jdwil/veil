//! HTTP harness for package `VeilRuntimeUI` (RT-001 / RT-003).
//! Wires adapters + exposes services as REST endpoints.
//! `cargo run -p veil_bin` from the generated workspace root.

use axum::{
    Json, Router,
    extract::Request,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{Next, from_fn},
    response::Response,
    routing::get,
};
use design_kit::application::{self as design_kit_app};
use runtime_u_i::application::{self as runtime_u_i_app};
use serde_json::Value;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;
use veil_shared::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);

    let app = Router::new().route("/health", get(|| async { "ok" }));
    println!("veil_bin: listening on :{}", port);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

/// Serialize for HTTP JSON, omitting fields annotated role:secret.
/// Persistence (repos) uses full `Serialize` — secrets still round-trip to storage.
fn veil_json_public<T: serde::Serialize>(value: &T) -> serde_json::Value {
    let mut v = serde_json::to_value(value).unwrap_or_default();
    veil_redact_secrets(&mut v);
    v
}

fn veil_redact_secrets(v: &mut serde_json::Value) {
    // Scalar secret fields from role:secret annotations (INV-001).
    const SECRET_KEYS: &[&str] = &[];
    // Header maps/lists often carry API keys — redact values, keep names.
    const HEADER_CONTAINERS: &[&str] = &["headers"];
    match v {
        serde_json::Value::Object(map) => {
            for k in SECRET_KEYS {
                map.remove(*k);
            }
            for hk in HEADER_CONTAINERS {
                if let Some(headers) = map.get_mut(*hk) {
                    veil_redact_header_values(headers);
                }
            }
            for (_k, child) in map.iter_mut() {
                veil_redact_secrets(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                veil_redact_secrets(item);
            }
        }
        _ => {}
    }
}

fn veil_redact_header_values(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                if let serde_json::Value::Object(h) = item {
                    if h.contains_key("value") {
                        h.insert("value".into(), serde_json::Value::String(String::new()));
                    }
                    if h.contains_key("Value") {
                        h.insert("Value".into(), serde_json::Value::String(String::new()));
                    }
                }
            }
        }
        serde_json::Value::Object(map) => {
            // Map-shaped headers: redact all values
            for (_k, val) in map.iter_mut() {
                *val = serde_json::Value::String(String::new());
            }
        }
        _ => {}
    }
}

fn veil_domain_error_status(e: DomainError) -> StatusCode {
    match &e {
        DomainError::NotFound => {
            eprintln!("warn: not found: {e}");
            StatusCode::NOT_FOUND
        }
        DomainError::Validation(msg) => {
            eprintln!("warn: validation: {msg}");
            StatusCode::BAD_REQUEST
        }
        DomainError::External(msg) => {
            eprintln!("error: upstream: {msg}");
            StatusCode::BAD_GATEWAY
        }
    }
}

/// Production-oriented auth:
/// - `/health` + OPTIONS always open
/// - `VEIL_DEV=1` → open (local dual-loop only)
/// - else require a key: `VEIL_API_KEY` (admin) and/or `VEIL_TENANT_KEYS`
///   (`tenant-uuid:secret,tenant-uuid2:secret2`)
/// - Present key via `X-Api-Key` or `Authorization: Bearer <key>`
/// - Tenant-scoped routes: prefer `X-Tenant-Id`; if key is a tenant key, that
///   UUID is forced (body/query tenant_id cannot elevate to another tenant)
async fn veil_api_key_middleware(
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.uri().path() == "/health" || request.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(request).await);
    }
    let dev = std::env::var("VEIL_DEV").ok().as_deref() == Some("1");
    let require = std::env::var("VEIL_REQUIRE_AUTH").ok().as_deref() == Some("1");
    let admin_key = std::env::var("VEIL_API_KEY").ok().filter(|s| !s.is_empty());
    let tenant_keys = veil_parse_tenant_keys();
    let presented = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        });

    if dev && !require && admin_key.is_none() && tenant_keys.is_empty() {
        return Ok(next.run(request).await);
    }

    let Some(presented) = presented else {
        eprintln!("error: missing X-Api-Key / Authorization Bearer");
        return Err(StatusCode::UNAUTHORIZED);
    };

    let is_admin = admin_key.as_deref() == Some(presented.as_str());
    let tenant_from_key = tenant_keys
        .iter()
        .find(|(_, k)| k == &presented)
        .map(|(t, _)| t.clone());

    if !is_admin && tenant_from_key.is_none() {
        eprintln!("warn: unauthorized — key not recognized");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let path = request.uri().path();
    // Provider catalog is admin-only (tenant keys cannot mutate global catalog).
    if path.starts_with("/api/providers") && !is_admin {
        eprintln!("warn: forbidden — provider catalog requires VEIL_API_KEY (admin)");
        return Err(StatusCode::FORBIDDEN);
    }
    // Tenant-scoped surfaces need either admin+X-Tenant-Id or a tenant key.
    let tenant_route = path.starts_with("/api/integrations") || path.starts_with("/api/execute");
    if tenant_route {
        if let Some(ref t) = tenant_from_key {
            if let Some(hdr) = headers.get("x-tenant-id").and_then(|v| v.to_str().ok()) {
                if hdr != t.as_str() {
                    eprintln!("warn: X-Tenant-Id does not match tenant API key");
                    return Err(StatusCode::FORBIDDEN);
                }
            }
        } else if is_admin {
            // Admin acting for a tenant must pass X-Tenant-Id (enforced in handler
            // when not VEIL_DEV).
        }
    }

    Ok(next.run(request).await)
}

/// `VEIL_TENANT_KEYS=uuid:key,uuid2:key2` → list of (tenant_id, api_key).
fn veil_parse_tenant_keys() -> Vec<(String, String)> {
    let Ok(raw) = std::env::var("VEIL_TENANT_KEYS") else {
        return Vec::new();
    };
    raw.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            let (t, k) = pair.split_once(':')?;
            let t = t.trim();
            let k = k.trim();
            if t.is_empty() || k.is_empty() {
                None
            } else {
                Some((t.to_string(), k.to_string()))
            }
        })
        .collect()
}

/// Resolve tenant_id for handlers: tenant API key wins; else X-Tenant-Id;
/// body/query only allowed in VEIL_DEV=1 when no tenant key.
fn veil_resolve_tenant_id(headers: &HeaderMap, fallback: Option<Uuid>) -> Result<Uuid, StatusCode> {
    let presented = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        });
    let tenant_keys = veil_parse_tenant_keys();
    if let Some(ref key) = presented {
        if let Some((tid, _)) = tenant_keys.iter().find(|(_, k)| k == key) {
            return tid
                .parse::<Uuid>()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    if let Some(Ok(s)) = headers.get("x-tenant-id").map(|v| v.to_str()) {
        let from_hdr = s.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;
        if let Some(fb) = fallback {
            if fb != from_hdr {
                eprintln!("warn: body/query tenant_id != X-Tenant-Id");
                return Err(StatusCode::FORBIDDEN);
            }
        }
        return Ok(from_hdr);
    }
    let dev = std::env::var("VEIL_DEV").ok().as_deref() == Some("1");
    if dev {
        return fallback.ok_or(StatusCode::BAD_REQUEST);
    }
    eprintln!("error: production requires X-Tenant-Id (or tenant API key)");
    Err(StatusCode::BAD_REQUEST)
}

/// Restrict CORS: `CORS_ORIGINS=http://a,http://b` or localhost defaults (not *).
fn veil_cors_layer() -> CorsLayer {
    use axum::http::{HeaderValue, Method};
    if let Ok(raw) = std::env::var("CORS_ORIGINS") {
        let origins: Vec<HeaderValue> = raw
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !origins.is_empty() {
            return CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers(Any);
        }
    }
    let local = [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:5174",
        "http://127.0.0.1:5174",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
    ];
    let origins: Vec<HeaderValue> = local.iter().filter_map(|s| s.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}

/// HTML `<input type="date">` and form empties → JSON values chrono/serde accept.
/// `""` → null; bare `YYYY-MM-DD` → `YYYY-MM-DDT00:00:00Z`.
fn veil_normalize_body_dt(v: Value) -> Value {
    match v {
        Value::String(s) if s.is_empty() => Value::Null,
        Value::String(s)
            if s.len() == 10
                && s.as_bytes().get(4) == Some(&b'-')
                && s.as_bytes().get(7) == Some(&b'-')
                && !s.contains('T') =>
        {
            Value::String(format!("{s}T00:00:00Z"))
        }
        other => other,
    }
}
