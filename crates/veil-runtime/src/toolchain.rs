//! Toolchain fingerprint — the load-bearing safety primitive for the FFI path.
//!
//! Rust has **no stable ABI**. A cdylib artifact `dlopen`'d into the execution
//! host MUST have been compiled with a compatible `rustc` (same version + target
//! triple) or the process risks undefined behaviour / a crash. Because the
//! host↔artifact seam is a JSON-bytes C ABI (`veil_workflow_run`) rather than a
//! shared Rust trait object, the practical blast radius is smaller than raw
//! trait-object sharing — but the spec (and `veil-execution-host-design`) treats
//! matching toolchains as a hard requirement, so we enforce it explicitly:
//!
//! 1. The host computes its own fingerprint at startup ([`host_fingerprint`]).
//! 2. Every artifact records the fingerprint it was compiled with.
//! 3. On load, the host **refuses** (typed error, no `dlopen`) if the artifact
//!    fingerprint is present and does not match the host fingerprint.
//!
//! A `None` artifact fingerprint (legacy record written before this field
//! existed) is permitted for backward compatibility but logged; new artifacts
//! written by `compile_and_register` always carry one.

use std::sync::OnceLock;

/// A compact, comparable description of the compiler + target an artifact (or
/// the host) was built with. Equality is exact-match: any drift is a refusal.
///
/// Serialized as a single string `"{rustc_version}/{target_triple}"` so it can
/// live on the artifact record and be compared cheaply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainFingerprint {
    /// `rustc` version string, e.g. `"1.79.0"` (from `rustc -vV` `release:`).
    pub rustc_version: String,
    /// Host target triple, e.g. `"x86_64-unknown-linux-gnu"`.
    pub target_triple: String,
}

impl ToolchainFingerprint {
    /// The canonical wire form stored on the artifact record.
    pub fn to_wire(&self) -> String {
        format!("{}/{}", self.rustc_version, self.target_triple)
    }

    /// Parse a wire-form fingerprint (`"{version}/{triple}"`).
    pub fn from_wire(s: &str) -> Option<Self> {
        let (v, t) = s.split_once('/')?;
        if v.is_empty() || t.is_empty() {
            return None;
        }
        Some(Self {
            rustc_version: v.to_string(),
            target_triple: t.to_string(),
        })
    }
}

impl std::fmt::Display for ToolchainFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_wire())
    }
}

/// The host's own toolchain fingerprint, computed once and cached.
///
/// Resolution order:
/// 1. `VEIL_TOOLCHAIN_FINGERPRINT` env override (wire form) — lets the deploy
///    image pin an exact value that matches the compile pipeline without a
///    live `rustc` in the runtime container.
/// 2. `rustc -vV` on PATH (`release:` line + `host:` line).
/// 3. Compile-time `RUSTC_VERSION`/target fallbacks baked via build cfg — if
///    `rustc` is unavailable we fall back to the target triple this binary was
///    compiled for and an `"unknown"` version (which will refuse to match any
///    real artifact fingerprint, failing safe).
pub fn host_fingerprint() -> &'static ToolchainFingerprint {
    static FP: OnceLock<ToolchainFingerprint> = OnceLock::new();
    FP.get_or_init(compute_host_fingerprint)
}

fn compute_host_fingerprint() -> ToolchainFingerprint {
    // 1. Explicit override (deploy image pins this to match the build pipeline).
    if let Ok(wire) = std::env::var("VEIL_TOOLCHAIN_FINGERPRINT") {
        if let Some(fp) = ToolchainFingerprint::from_wire(wire.trim()) {
            return fp;
        }
    }

    // 2. Ask `rustc` directly.
    if let Some(fp) = rustc_fingerprint() {
        return fp;
    }

    // 3. Fail-safe fallback: known target triple, unknown version. An unknown
    //    version never equals a real artifact fingerprint, so loads refuse
    //    rather than risk an ABI mismatch.
    ToolchainFingerprint {
        rustc_version: "unknown".to_string(),
        target_triple: current_target_triple(),
    }
}

/// Run `rustc -vV` and parse the `release:` and `host:` lines.
fn rustc_fingerprint() -> Option<ToolchainFingerprint> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = std::process::Command::new(rustc).arg("-vV").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut version = None;
    let mut host = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("release:") {
            version = Some(v.trim().to_string());
        } else if let Some(h) = line.strip_prefix("host:") {
            host = Some(h.trim().to_string());
        }
    }
    match (version, host) {
        (Some(v), Some(h)) if !v.is_empty() && !h.is_empty() => Some(ToolchainFingerprint {
            rustc_version: v,
            target_triple: h,
        }),
        _ => None,
    }
}

/// The target triple this binary was compiled for. Derived from `cfg` so it is
/// always available even without a live `rustc`.
fn current_target_triple() -> String {
    // Compose from the compile-time target cfg attributes.
    let arch = std::env::consts::ARCH; // e.g. "x86_64", "aarch64"
    let os = std::env::consts::OS; // e.g. "linux", "macos"
    // A best-effort triple; the env override / rustc path give exact values.
    match os {
        "linux" => format!("{arch}-unknown-linux-gnu"),
        "macos" => format!("{arch}-apple-darwin"),
        other => format!("{arch}-unknown-{other}"),
    }
}

/// Error returned when an artifact's toolchain fingerprint does not match the
/// host's. This is a **refuse-to-load** condition, not a soft warning.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "toolchain fingerprint mismatch: artifact built with '{artifact}', host is '{host}' — refusing to dlopen (Rust has no stable ABI)"
)]
pub struct FingerprintMismatch {
    pub artifact: String,
    pub host: String,
}

/// Check an artifact's recorded fingerprint against the host. A `None` artifact
/// fingerprint is a legacy record and is permitted (logged by the caller).
///
/// Returns `Err(FingerprintMismatch)` only when a fingerprint is present and
/// differs from the host — the case that must refuse the load.
pub fn check_compatible(artifact_fingerprint: Option<&str>) -> Result<(), FingerprintMismatch> {
    let host = host_fingerprint();
    match artifact_fingerprint {
        None => Ok(()),
        Some(wire) => {
            let host_wire = host.to_wire();
            if wire == host_wire {
                Ok(())
            } else {
                Err(FingerprintMismatch {
                    artifact: wire.to_string(),
                    host: host_wire,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trips() {
        let fp = ToolchainFingerprint {
            rustc_version: "1.79.0".into(),
            target_triple: "x86_64-unknown-linux-gnu".into(),
        };
        let wire = fp.to_wire();
        assert_eq!(wire, "1.79.0/x86_64-unknown-linux-gnu");
        assert_eq!(ToolchainFingerprint::from_wire(&wire), Some(fp));
    }

    #[test]
    fn from_wire_rejects_malformed() {
        assert_eq!(ToolchainFingerprint::from_wire("no-slash"), None);
        assert_eq!(ToolchainFingerprint::from_wire("/only-triple"), None);
        assert_eq!(ToolchainFingerprint::from_wire("version-only/"), None);
    }

    #[test]
    fn matching_fingerprint_is_compatible() {
        let host = host_fingerprint().to_wire();
        assert!(check_compatible(Some(&host)).is_ok());
    }

    #[test]
    fn mismatched_fingerprint_is_refused() {
        // A fingerprint that cannot possibly match the host.
        let bogus = "0.0.0-bogus/mips-unknown-none";
        let err = check_compatible(Some(bogus)).unwrap_err();
        assert_eq!(err.artifact, bogus);
        assert!(err.to_string().contains("fingerprint mismatch"));
    }

    #[test]
    fn missing_fingerprint_is_permitted_for_legacy() {
        assert!(check_compatible(None).is_ok());
    }

    #[test]
    fn host_fingerprint_is_stable_across_calls() {
        let a = host_fingerprint();
        let b = host_fingerprint();
        assert_eq!(a, b);
    }
}
