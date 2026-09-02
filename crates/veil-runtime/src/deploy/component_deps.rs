//! Cross-project UI component dependency resolution for UI builds.
//!
//! A VEIL UI project can `use <layer>` to gain *vocabulary* for composite
//! components (e.g. `CrudResource`, `CollectionView`) that are actually
//! *implemented* in a SEPARATE library project. When the consumer is generated,
//! codegen emits `import X from '$lib/components/X.svelte'` into its pages — but
//! that `.svelte` only exists in the library project's generated output, never in
//! the consumer's build tree. Vite/rollup then fails to resolve the import.
//!
//! This module closes that gap generically and data-driven:
//!   1. A vocabulary layer declares its implementing project + exported component
//!      names via `implemented_by <slug>` + `provides <Comp> …` (parsed in
//!      veil-ir as [`veil_ir::ComponentProvider`]).
//!   2. Before the consumer's vite build, we resolve which of the consumer's
//!      `use`d layers are component providers, fetch each implementing project
//!      from the store, `veil gen` it, and copy the declared exported
//!      `src/lib/components/<Name>.svelte` (plus any transitively-imported
//!      `$lib/components/*.svelte`) into the consumer's generated tree at the
//!      SAME path the imports expect.
//!
//! **No project names are hardcoded here** — everything is driven by the layer's
//! declaration + the store. A different library project with its own layer works
//! identically with zero engine changes.
//!
//! Flow:
//!   - [`resolve_component_deps`] (needs store `Deps`): read the consumer source,
//!     resolve provider layers, materialize each provider project's source to a
//!     temp dir, and return the list of [`ComponentDep`].
//!   - [`materialize_component_deps`] (pure filesystem, no store): for each dep,
//!     `veil gen` the provider source and copy its exported components into the
//!     consumer's generated `src/lib/components/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tracing::{info, warn};

/// A resolved cross-project component dependency: a provider project whose
/// exported Svelte components must be materialized into the consumer's build.
#[derive(Debug, Clone)]
pub struct ComponentDep {
    /// The provider project slug (from the layer's `implemented_by`).
    pub provider_slug: String,
    /// The layer that declared this provider (for diagnostics/logging).
    pub layer_name: String,
    /// Exported component names the provider makes available.
    pub provides: Vec<String>,
    /// On-disk directory containing the provider project's materialized source
    /// (its `.veil`, `layers/`, etc.). Used to `veil gen` the provider.
    pub source_dir: PathBuf,
    /// The provider's entry `.veil` file name (relative to `source_dir`).
    pub veil_file: String,
}

/// Read the consumer's UI source and resolve the set of cross-project component
/// dependencies it declares (via component-provider layers it `use`s).
///
/// For each provider, the implementing project's full source is materialized
/// from the store into a temp dir so it can later be generated. Returns an empty
/// vec when the consumer has no external component deps (regression-safe: such a
/// project builds unchanged).
///
/// `deps` is the storage layer, `consumer_source_dir` the already-materialized
/// consumer project source, `consumer_veil_file` the consumer entry (e.g.
/// `ui.veil`).
pub async fn resolve_component_deps(
    deps: &storage::application::Deps,
    consumer_source_dir: &Path,
    consumer_veil_file: &str,
) -> Vec<ComponentDep> {
    let veil_path = consumer_source_dir.join(consumer_veil_file);
    let src = match tokio::fs::read_to_string(&veil_path).await {
        Ok(s) => s,
        Err(e) => {
            warn!(
                path = %veil_path.display(),
                "component-deps: cannot read consumer veil file: {e}"
            );
            return Vec::new();
        }
    };

    let use_names = veil_ir::collect_veil_use_names(&src);
    let mut deps_out: Vec<ComponentDep> = Vec::new();

    for layer_name in use_names {
        // Resolve the layer body: product layer materialized into the consumer
        // source, else a platform layer from the catalog.
        let Some(layer_content) = resolve_layer_content(consumer_source_dir, &layer_name) else {
            continue;
        };

        // Does this layer declare a component provider? (data-driven; no names)
        let Some(provider) = veil_ir::parse_layer_component_provider(&layer_content) else {
            continue;
        };

        info!(
            layer = %layer_name,
            provider = %provider.implemented_by,
            n_components = provider.provides.len(),
            "component-deps: layer declares a component provider"
        );

        // Materialize the implementing project's source from the store.
        match materialize_provider_source(deps, &provider.implemented_by).await {
            Ok(Some((source_dir, veil_file))) => {
                deps_out.push(ComponentDep {
                    provider_slug: provider.implemented_by,
                    layer_name,
                    provides: provider.provides,
                    source_dir,
                    veil_file,
                });
            }
            Ok(None) => {
                warn!(
                    provider = %provider.implemented_by,
                    "component-deps: provider project has no .veil entry file; skipping"
                );
            }
            Err(e) => {
                warn!(
                    provider = %provider.implemented_by,
                    "component-deps: failed to materialize provider source: {e}"
                );
            }
        }
    }

    deps_out
}

/// Resolve a layer's raw content for a consumer. Prefers a product layer
/// materialized into the consumer source tree (`layers/<name>.layer` or
/// `<name>.layer`), falling back to the platform layer catalog.
fn resolve_layer_content(consumer_source_dir: &Path, layer_name: &str) -> Option<String> {
    let candidates = [
        consumer_source_dir
            .join("layers")
            .join(format!("{layer_name}.layer")),
        consumer_source_dir.join(format!("{layer_name}.layer")),
    ];
    for path in candidates {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
    }
    // Platform catalog (VEIL-owned layers).
    veil_ir::resolve_platform_layer_content(layer_name)
}

/// Candidate entry `.veil` file names for a provider project, in priority order.
/// A provider (library) project's stock components typically live in `main.veil`;
/// some use `ui.veil`. As a last resort we scan for any root-level `.veil`.
const PROVIDER_VEIL_CANDIDATES: &[&str] = &["main.veil", "ui.veil", "app.veil"];

/// Fetch a provider project's full source from the store into a stable temp dir.
/// Returns `Ok(Some((source_dir, veil_file)))` on success, `Ok(None)` when the
/// project exists but has no discoverable `.veil` entry.
async fn materialize_provider_source(
    deps: &storage::application::Deps,
    provider_slug: &str,
) -> Result<Option<(PathBuf, String)>, String> {
    // Resolve slug → repo id.
    let repo = storage::application::resolve_repo(deps, provider_slug)
        .await
        .map_err(|e| format!("resolve provider '{provider_slug}': {e:?}"))?;
    let repo_id = repo.id.value;
    let rid = || storage::domain::types::RepoId {
        value: repo_id.clone(),
    };

    let dir = std::path::PathBuf::from(format!(
        "/tmp/deploy/_component_providers/{provider_slug}"
    ));
    // Fresh each build to avoid stale files (idempotent, safe).
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("mkdir provider source dir: {e}"))?;

    let files = storage::application::list_files(deps, rid(), "main".to_string(), String::new())
        .await
        .map_err(|e| format!("list provider files: {e:?}"))?;

    for file_path in &files {
        if let Ok(fbytes) =
            storage::application::read_file(deps, rid(), "main".to_string(), file_path.clone()).await
        {
            let dest = dir.join(file_path);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            tokio::fs::write(&dest, &fbytes).await.ok();
        }
    }

    Ok(pick_provider_veil_file(&files).map(|vf| (dir, vf)))
}

/// Pick the provider entry `.veil` from its file list (data-driven, no project
/// names). Prefers the well-known candidates, else the first root-level `.veil`.
fn pick_provider_veil_file(files: &[String]) -> Option<String> {
    for cand in PROVIDER_VEIL_CANDIDATES {
        if files.iter().any(|f| f == cand) {
            return Some((*cand).to_string());
        }
    }
    // First root-level (no slash) .veil.
    files
        .iter()
        .find(|f| f.ends_with(".veil") && !f.contains('/'))
        .cloned()
}

/// Generate each provider project and copy its exported Svelte components
/// (plus any transitively-imported `$lib/components/*.svelte`) into the
/// consumer's generated `src/lib/components/`.
///
/// `consumer_gen_dir` is the consumer's `veil gen` output dir (where its own
/// `src/lib/components/` already lives). Pure filesystem + `veil` CLI; no store
/// access. Idempotent: only the declared exports and their component imports are
/// copied, unrelated files are left untouched.
pub async fn materialize_component_deps(
    consumer_gen_dir: &Path,
    deps: &[ComponentDep],
) -> Result<(), String> {
    if deps.is_empty() {
        return Ok(());
    }

    let consumer_components = consumer_gen_dir.join("src/lib/components");
    tokio::fs::create_dir_all(&consumer_components)
        .await
        .map_err(|e| format!("mkdir consumer components dir: {e}"))?;

    for dep in deps {
        // Generate the provider into a temp dir alongside its source.
        let provider_gen = dep.source_dir.join("__component_gen");
        let _ = tokio::fs::remove_dir_all(&provider_gen).await;
        tokio::fs::create_dir_all(&provider_gen)
            .await
            .map_err(|e| format!("mkdir provider gen dir: {e}"))?;

        let veil_path = dep.source_dir.join(&dep.veil_file);
        run_veil_gen(&veil_path, &provider_gen).await.map_err(|e| {
            format!(
                "veil gen provider '{}' ({}): {e}",
                dep.provider_slug, dep.veil_file
            )
        })?;

        let provider_components = provider_gen.join("src/lib/components");
        if !provider_components.exists() {
            warn!(
                provider = %dep.provider_slug,
                "component-deps: provider generated no src/lib/components; nothing to copy"
            );
            continue;
        }

        let copied = copy_exported_components(
            &consumer_components,
            &provider_components,
            &dep.provides,
        )
        .await?;
        for name in &copied {
            info!(
                provider = %dep.provider_slug,
                component = %name,
                "component-deps: materialized component into consumer"
            );
        }

        // Materialize the provider layer's GLOBAL CSS (tokens + shared classes).
        // Framework-neutral: a project's layer-level global CSS is emitted to a
        // plain `src/app.css` by codegen regardless of target (raw HTML/CSS,
        // Svelte, React, Vue, …). The provider's components reference those
        // tokens/classes, so without this the consumer bundle has the markup but
        // no styles. Append the provider's app.css into the consumer's app.css
        // (deduped by a provider marker) so the consumer's build bundles it.
        let provider_css = provider_gen.join("src/app.css");
        if provider_css.exists() {
            if let Ok(css) = tokio::fs::read_to_string(&provider_css).await {
                let marker = format!("/* veil:component-dep {} */", dep.provider_slug);
                let consumer_css_path = consumer_gen_dir.join("src/app.css");
                let existing = tokio::fs::read_to_string(&consumer_css_path)
                    .await
                    .unwrap_or_default();
                if !existing.contains(&marker) {
                    let merged = format!("{existing}\n{marker}\n{css}\n");
                    if let Err(e) = tokio::fs::write(&consumer_css_path, merged).await {
                        warn!(provider = %dep.provider_slug, "component-deps: failed to merge provider app.css: {e}");
                    } else {
                        info!(provider = %dep.provider_slug, "component-deps: merged provider global CSS into consumer app.css");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Copy the declared exported components (plus any transitively-imported
/// `$lib/components/*.svelte`) from a provider's generated components dir into
/// the consumer's components dir. Returns the names actually written.
///
/// Pure filesystem, no store or `veil` CLI — the unit of behavior that makes a
/// consumer's cross-project imports resolve. Does not clobber a component the
/// consumer authored itself, and silently skips declared components missing from
/// the provider (logged by the caller via the returned set).
pub async fn copy_exported_components(
    consumer_components: &Path,
    provider_components: &Path,
    provides: &[String],
) -> Result<Vec<String>, String> {
    tokio::fs::create_dir_all(consumer_components)
        .await
        .map_err(|e| format!("mkdir consumer components dir: {e}"))?;

    let mut queue: Vec<String> = provides.to_vec();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut written: Vec<String> = Vec::new();

    while let Some(name) = queue.pop() {
        if seen.contains(&name) {
            continue;
        }
        seen.insert(name.clone());

        let src_file = provider_components.join(format!("{name}.svelte"));
        let src_content = match tokio::fs::read_to_string(&src_file).await {
            Ok(c) => c,
            Err(_) => {
                warn!(
                    component = %name,
                    "component-deps: declared/transitive component not found in provider gen; skipping"
                );
                continue;
            }
        };

        let dest_file = consumer_components.join(format!("{name}.svelte"));
        // Don't clobber a component the consumer authored itself.
        if !dest_file.exists() {
            tokio::fs::write(&dest_file, &src_content)
                .await
                .map_err(|e| format!("write component {name}.svelte: {e}"))?;
            written.push(name.clone());
        }

        // Follow transitive `$lib/components/<X>.svelte` imports.
        for transitive in extract_lib_component_imports(&src_content) {
            if !seen.contains(&transitive) {
                queue.push(transitive);
            }
        }
    }

    Ok(written)
}

/// Run `veil gen <veil_path> -t typescript -o <out_dir>`.
async fn run_veil_gen(veil_path: &Path, out_dir: &Path) -> Result<(), String> {
    let output = Command::new("veil")
        .args([
            "gen",
            &veil_path.to_string_lossy(),
            "-t",
            "typescript",
            "-o",
            &out_dir.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("veil gen failed to start: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("veil gen non-zero: {stderr}"));
    }
    Ok(())
}

/// Extract component names imported via `$lib/components/<Name>.svelte` from a
/// Svelte file's source. Used to follow transitive shared-component deps between
/// composites in the same provider library.
pub fn extract_lib_component_imports(content: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let needle = "$lib/components/";
    for (idx, _) in content.match_indices(needle) {
        let after = &content[idx + needle.len()..];
        if let Some(svelte_pos) = after.find(".svelte") {
            let name = &after[..svelte_pos];
            // Guard: PascalCase component file name only (no path separators).
            if !name.is_empty()
                && !name.contains('/')
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false)
            {
                out.insert(name.to_string());
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_lib_imports_finds_pascal_components() {
        let src = r#"
            <script>
              import CrudResource from '$lib/components/CrudResource.svelte';
              import StatusPill from "$lib/components/StatusPill.svelte";
              import { helper } from '$lib/util';
            </script>
        "#;
        let mut got = extract_lib_component_imports(src);
        got.sort();
        assert_eq!(got, vec!["CrudResource", "StatusPill"]);
    }

    #[test]
    fn extract_lib_imports_ignores_non_component_paths() {
        let src = "import x from '$lib/components/nested/thing.svelte';\n\
                   import y from '$lib/stores/foo.svelte';";
        // nested/thing has a slash → skipped; foo is not under components → skipped
        assert!(extract_lib_component_imports(src).is_empty());
    }

    #[test]
    fn pick_provider_veil_prefers_main() {
        let files = vec![
            "veil.toml".to_string(),
            "ui.veil".to_string(),
            "main.veil".to_string(),
        ];
        assert_eq!(pick_provider_veil_file(&files), Some("main.veil".to_string()));
    }

    #[test]
    fn pick_provider_veil_falls_back_to_root_veil() {
        let files = vec![
            "veil.toml".to_string(),
            "layers/designkit.layer".to_string(),
            "custom.veil".to_string(),
        ];
        assert_eq!(
            pick_provider_veil_file(&files),
            Some("custom.veil".to_string())
        );
    }

    #[test]
    fn pick_provider_veil_none_when_no_veil() {
        let files = vec!["veil.toml".to_string(), "README.md".to_string()];
        assert_eq!(pick_provider_veil_file(&files), None);
    }

    /// Unique temp dir per test invocation.
    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "veil-compdeps-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Proves a consumer that uses a component-vocabulary layer gets the declared
    /// components (plus a transitive one they import) materialized into its
    /// generated `src/lib/components/`. NOTE: uses a GENERIC fixture library
    /// ("widgetkit"/"Gadget…") — no 'designkit' anywhere — proving the engine is
    /// data-driven, not hardcoded.
    #[tokio::test]
    async fn materializes_declared_and_transitive_components_generically() {
        let root = tmp("materialize");
        let provider_components = root.join("provider/src/lib/components");
        let consumer_components = root.join("consumer/src/lib/components");
        tokio::fs::create_dir_all(&provider_components).await.unwrap();
        tokio::fs::create_dir_all(&consumer_components).await.unwrap();

        // Provider library exports GadgetList, which internally imports the
        // (undeclared, transitive) GadgetBadge component.
        tokio::fs::write(
            provider_components.join("GadgetList.svelte"),
            "<script>import GadgetBadge from '$lib/components/GadgetBadge.svelte';</script>\n<div class=\"gadget-list\"></div>\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            provider_components.join("GadgetBadge.svelte"),
            "<span class=\"gadget-badge\"></span>\n",
        )
        .await
        .unwrap();
        // An unrelated file that must NOT be copied (only declared+transitive).
        tokio::fs::write(
            provider_components.join("UnrelatedThing.svelte"),
            "<div>nope</div>\n",
        )
        .await
        .unwrap();

        // Consumer declares it wants GadgetList (transitive GadgetBadge follows).
        let written = copy_exported_components(
            &consumer_components,
            &provider_components,
            &["GadgetList".to_string()],
        )
        .await
        .unwrap();

        assert!(consumer_components.join("GadgetList.svelte").exists());
        assert!(
            consumer_components.join("GadgetBadge.svelte").exists(),
            "transitive component must be materialized"
        );
        assert!(
            !consumer_components.join("UnrelatedThing.svelte").exists(),
            "unrelated provider files must not be swept in"
        );
        assert!(written.contains(&"GadgetList".to_string()));
        assert!(written.contains(&"GadgetBadge".to_string()));

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// Consumer-authored components are not clobbered by the provider's copy.
    #[tokio::test]
    async fn does_not_clobber_consumer_authored_component() {
        let root = tmp("noclobber");
        let provider_components = root.join("provider/src/lib/components");
        let consumer_components = root.join("consumer/src/lib/components");
        tokio::fs::create_dir_all(&provider_components).await.unwrap();
        tokio::fs::create_dir_all(&consumer_components).await.unwrap();

        tokio::fs::write(
            provider_components.join("Shared.svelte"),
            "<div>from-provider</div>\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            consumer_components.join("Shared.svelte"),
            "<div>consumer-owned</div>\n",
        )
        .await
        .unwrap();

        let written = copy_exported_components(
            &consumer_components,
            &provider_components,
            &["Shared".to_string()],
        )
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(consumer_components.join("Shared.svelte"))
            .await
            .unwrap();
        assert_eq!(content.trim(), "<div>consumer-owned</div>");
        assert!(written.is_empty(), "must not report clobbered write");

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    /// End-to-end resolution wiring (without the store): a generic vocabulary
    /// layer materialized into the consumer source declares a provider, and the
    /// engine resolves it purely from the layer content — no project names in
    /// the engine.
    #[test]
    fn resolves_provider_from_generic_layer_in_consumer_source() {
        let root = tmp("resolvelayer");
        let layers = root.join("layers");
        std::fs::create_dir_all(&layers).unwrap();
        std::fs::write(
            layers.join("widgetkit.layer"),
            "pkg widgetkit v1\n  implemented_by acme-widgetkit\n  provides GadgetList GadgetBadge\n",
        )
        .unwrap();

        let content = resolve_layer_content(&root, "widgetkit").expect("layer resolved");
        let provider = veil_ir::parse_layer_component_provider(&content).expect("provider");
        assert_eq!(provider.implemented_by, "acme-widgetkit");
        assert_eq!(provider.provides, vec!["GadgetList", "GadgetBadge"]);

        // A layer with no provider declaration resolves to no provider.
        std::fs::write(
            layers.join("plainkit.layer"),
            "pkg plainkit v1\n  construct Foo\n    kw foo\n    mt struct\n",
        )
        .unwrap();
        let plain = resolve_layer_content(&root, "plainkit").expect("layer resolved");
        assert!(veil_ir::parse_layer_component_provider(&plain).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Regression safety: materialize with zero deps is a clean no-op.
    #[tokio::test]
    async fn empty_deps_is_noop() {
        let root = tmp("noop");
        let gen_dir = root.join("generated");
        tokio::fs::create_dir_all(&gen_dir).await.unwrap();
        materialize_component_deps(&gen_dir, &[]).await.unwrap();
        // No components dir needs to exist for a no-op.
        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}
