//! Platform (VEIL-owned) layer catalog paths and name policy.
//!
//! **Platform layers** (`ddd`, `di`, …) ship with VEIL and are **read-only** for product
//! coders. Teams customize by forking under a new name (`acme-ddd.layer`) in the product.
//!
//! Resolution sources for platform names (in order):
//! 1. `VEIL_LAYERS_DIR`
//! 2. `$TMP/veil-platform-layers` (DDB+S3 materialize cache)
//! 3. Install layout next to the binary
//! 4. Monorepo `layers/` found from CWD / exe (dev only — direct `layers/<name>.layer`)
//!
//! Product packages never shadow platform names via session materialization under `/tmp`.

use std::path::PathBuf;

/// Names owned by the VEIL platform install (not product source trees).
///
/// Product `use ddd` always resolves here. To customize, copy to e.g. `acme-ddd.layer`
/// and `use acme-ddd`.
pub fn is_platform_layer_name(stem: &str) -> bool {
    matches!(
        stem,
        "base"
            | "ddd"
            | "di"
            | "functional"
            | "rust"
            | "harness"
            | "ui"
            | "svelte5"
            | "sveltekit5"
            | "transports"
            | "rig"
            | "aws_storage"
            | "rest_english"
            | "rest_rpc"
            | "bus_handle"
            | "auth_local"
            | "designkit"
    )
}

/// Default cache dir for DDB+S3 materialized platform layers.
pub fn platform_layers_cache_dir() -> PathBuf {
    std::env::temp_dir().join("veil-platform-layers")
}

/// Directories that may contain platform `*.layer` files (existing paths only).
pub fn platform_layer_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(d) = std::env::var("VEIL_LAYERS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            dirs.push(p);
        }
    }

    let cache = platform_layers_cache_dir();
    if cache.is_dir() && !dirs.iter().any(|d| d == &cache) {
        dirs.push(cache);
    }

    // Install layout next to binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for rel in ["../layers", "layers", "../../../layers"] {
                let p = exe_dir.join(rel);
                if p.is_dir() && !dirs.iter().any(|d| d == &p) {
                    dirs.push(p);
                }
            }
        }
    }

    // Dev: monorepo `layers/` from CWD ancestors (must contain a known platform file)
    if let Ok(cwd) = std::env::current_dir() {
        for anc in cwd.ancestors() {
            let p = anc.join("layers");
            if p.is_dir()
                && p.join("ddd.layer").is_file()
                && !dirs.iter().any(|d| d == &p)
            {
                dirs.push(p);
                break;
            }
        }
    }

    dirs
}

/// Read platform layer body by name from catalog dirs only.
pub fn resolve_platform_layer_content(name: &str) -> Option<String> {
    for dir in platform_layer_dirs() {
        let path = dir.join(format!("{name}.layer"));
        if path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    return Some(content);
                }
            }
        }
    }
    None
}

/// Whether ambient sibling-product layer discovery is enabled.
///
/// Off for cloud (`VEIL_SOURCE_MODE=s3`). On for pure disk hub, or when
/// `VEIL_LAYER_SIBLING_SCAN=1`.
pub fn sibling_product_layer_scan_enabled() -> bool {
    if std::env::var("VEIL_LAYER_SIBLING_SCAN")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return true;
    }
    matches!(
        std::env::var("VEIL_SOURCE_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "disk"
    )
}

/// True when layer body is a ghost stub (e.g. unit-test `pkg ddd v1\n`).
pub fn is_ghost_layer_content(content: &str) -> bool {
    let t = content.trim();
    if t.is_empty() {
        return true;
    }
    // Only a pkg header (optionally with desc/author) and no constructs/prompts.
    let has_construct = t.lines().any(|l| {
        let s = l.trim_start();
        s.starts_with("construct ")
            || s.starts_with("statement ")
            || s.starts_with("prompt")
            || s.starts_with("declare")
    });
    !has_construct && t.len() < 512
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_include_ddd() {
        assert!(is_platform_layer_name("ddd"));
        assert!(is_platform_layer_name("di"));
        assert!(!is_platform_layer_name("agent-registry"));
        assert!(!is_platform_layer_name("acme-ddd"));
    }

    #[test]
    fn ghost_detection() {
        assert!(is_ghost_layer_content("pkg ddd v1\n"));
        assert!(is_ghost_layer_content("pkg ddd v1\n  desc \"x\"\n"));
        assert!(!is_ghost_layer_content(
            "pkg ddd v1\n  construct Aggregate\n    kw agg\n    mt struct\n"
        ));
    }

    #[test]
    fn platform_ddd_not_shadowed_by_product_ghost_under_tmp() {
        use crate::LayerRegistry;

        let layers = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../layers");
        assert!(
            layers.join("ddd.layer").is_file(),
            "expected monorepo ddd.layer at {}",
            layers.display()
        );
        unsafe {
            std::env::set_var("VEIL_LAYERS_DIR", &layers);
            std::env::set_var("VEIL_SOURCE_MODE", "s3");
        }

        // Product session-like tree with a ghost layers/ddd.layer (must not win).
        let tmp = std::env::temp_dir().join(format!("veil-plat-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(tmp.join("layers"));
        std::fs::write(tmp.join("layers/ddd.layer"), "pkg ddd v1\n").unwrap();
        std::fs::write(
            tmp.join("main.veil"),
            "pkg T\n  use ddd\n\n  agg Foo\n    root\n      id: Str\n",
        )
        .unwrap();

        let reg = LayerRegistry::for_veil_file(&tmp.join("main.veil")).expect("registry");
        assert!(
            reg.construct("agg").is_some(),
            "agg missing after platform resolve; layers={:?} n_constructs={}",
            reg.layers,
            reg.constructs.len()
        );
        assert!(
            !reg.prompts.is_empty(),
            "ddd layer prompt must be present"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
