//! Generate loader structs for VEIL project links (shared-object loading).
//!
//! When a project declares `link dlx_auth`, this module generates:
//! 1. A `LinkedDlxAuth` struct with trait-object fields for each port the library provides
//! 2. A `load(path)` constructor that uses `libloading` to open the .so and call factory functions
//! 3. The loaded trait objects are then wired into the consumer's Deps struct
//!
//! The linked library is compiled as a cdylib and exports `#[no_mangle] extern "C"` factory
//! functions named `create_{snake_port_name}` that return `*mut dyn PortTrait`.

use veil_ir::ast::{Construct, LinkDecl, Solution, TopLevelItem};
use veil_ir::layer::{LayerRegistry, Shape};

/// Information about a linked project: which ports/traits it provides.
#[derive(Debug, Clone)]
pub struct LinkedProjectInfo {
    /// The link declaration.
    pub link: LinkDecl,
    /// PascalCase struct name for the loader (e.g. `LinkedDlxAuth`).
    pub loader_struct_name: String,
    /// Snake_case name for the env var (e.g. `DLX_AUTH`).
    pub env_name: String,
    /// Snake_case slug (e.g. `dlx_auth`).
    pub slug: String,
    /// Trait names the library provides (e.g. `["JwksProvider", "SessionStore"]`).
    pub port_traits: Vec<String>,
}

/// Collect info about all VEIL project links in the solution.
/// For each project link, determine which port traits it provides by
/// looking at layer-provided traits whose `@library_source` matches.
pub fn collect_linked_projects(
    solution: &Solution,
    registry: &LayerRegistry,
) -> Vec<LinkedProjectInfo> {
    solution
        .links
        .iter()
        .filter(|l| l.is_project_link)
        .map(|link| {
            let slug = link.name.replace('-', "_");
            let pascal = to_pascal_case(&slug);
            let env_name = slug.to_uppercase();

            // Discover which port traits this library provides.
            // Strategy: look for traits annotated with @library_source matching this link name,
            // OR traits from the layer with the same name as this link.
            let port_traits = discover_port_traits_for_link(solution, registry, &link.name);

            LinkedProjectInfo {
                link: link.clone(),
                loader_struct_name: format!("Linked{}", pascal),
                env_name,
                slug,
                port_traits,
            }
        })
        .collect()
}

/// Discover which port traits a linked library provides.
/// Looks at:
/// 1. Constructs with `@library_source(link_name)` annotation
/// 2. Layer-provided Trait constructs from the layer whose name matches link_name
fn discover_port_traits_for_link(
    solution: &Solution,
    registry: &LayerRegistry,
    link_name: &str,
) -> Vec<String> {
    let mut traits = Vec::new();
    let normalized = link_name.replace('-', "_");

    // Check solution items for layer-provided traits from this library
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Trait && c.layer_provided {
                // Check @library_source annotation
                if has_library_source_annotation(c, &normalized) {
                    traits.push(c.name.clone());
                }
            }
        }
    }

    // Also check layer registry for traits declared by a layer with matching name
    if traits.is_empty() {
        for spec in &registry.constructs {
            if spec.shape == Shape::Trait {
                // Match by layer name: the layer registry stores the layer name
                // and constructs declared within it.
                let layer_norm = spec.layer.replace('-', "_");
                if layer_norm == normalized {
                    traits.push(spec.name.clone());
                }
            }
        }
    }

    traits
}

/// Check if a construct has a `@library_source(name)` annotation matching the link.
fn has_library_source_annotation(c: &Construct, normalized_link_name: &str) -> bool {
    c.annotations.iter().any(|ann| {
        if ann.name == "library_source" {
            ann.args.iter().any(|arg| {
                arg.replace('-', "_") == normalized_link_name
            })
        } else {
            false
        }
    })
}

/// Generate the loader module file for all project links.
/// Returns `(file_path, content)` pairs for each linked loader.
pub fn gen_linked_loader_module(
    linked_projects: &[LinkedProjectInfo],
) -> Option<(String, String)> {
    if linked_projects.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(2048);
    out.push_str("//! Auto-generated shared object loaders for linked VEIL projects.\n");
    out.push_str("//!\n");
    out.push_str("//! Each loader opens a .so/.dylib via `libloading` and calls factory\n");
    out.push_str("//! functions to obtain trait objects. Zero serialization at the boundary.\n\n");
    out.push_str("#![allow(unsafe_code, unused_imports)]\n\n");
    out.push_str("use libloading::Library;\n");
    out.push_str("use std::sync::Arc;\n");
    out.push_str("use crate::ports::*;\n\n");

    for proj in linked_projects {
        gen_loader_struct(&mut out, proj);
    }

    Some(("crates/veil_shared/src/linked_loaders.rs".to_string(), out))
}

/// Generate a single loader struct + impl for one linked project.
fn gen_loader_struct(out: &mut String, proj: &LinkedProjectInfo) {
    let struct_name = &proj.loader_struct_name;

    // Struct definition with _lib handle + trait object fields
    out.push_str(&format!(
        "/// Loader for the `{}` shared library.\n",
        proj.link.name
    ));
    out.push_str(&format!(
        "/// Loads `lib{}.so` and calls factory functions for each port.\n",
        proj.slug
    ));
    out.push_str(&format!("pub struct {} {{\n", struct_name));
    out.push_str("    _lib: Library,\n");
    for trait_name in &proj.port_traits {
        let field = to_snake_case(trait_name);
        out.push_str(&format!(
            "    pub {}: Box<dyn {} + Send + Sync>,\n",
            field, trait_name
        ));
    }
    out.push_str("}\n\n");

    // impl with load() constructor
    out.push_str(&format!("impl {} {{\n", struct_name));
    out.push_str("    /// Load the shared library and instantiate all port trait objects.\n");
    out.push_str("    ///\n");
    out.push_str(&format!(
        "    /// The library path can be set via `{}_LIB_PATH` env var,\n",
        proj.env_name
    ));
    out.push_str(&format!(
        "    /// or defaults to `./libs/lib{}.so`.\n",
        proj.slug
    ));
    out.push_str(
        "    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {\n",
    );
    out.push_str("        let lib = unsafe { Library::new(path)? };\n");

    // Load each trait via factory function
    for trait_name in &proj.port_traits {
        let field = to_snake_case(trait_name);
        let factory_name = format!("create_{}", field);
        out.push_str(&format!(
            "        let {} = unsafe {{\n",
            field
        ));
        out.push_str(&format!(
            "            let factory: libloading::Symbol<unsafe extern \"C\" fn() -> *mut (dyn {} + Send + Sync)>\n",
            trait_name
        ));
        out.push_str(&format!(
            "                = lib.get(b\"{}\")?;\n",
            factory_name
        ));
        out.push_str(&format!(
            "            Box::from_raw(factory())\n"
        ));
        out.push_str("        };\n");
    }

    // Construct Self
    out.push_str("        Ok(Self {\n");
    out.push_str("            _lib: lib,\n");
    for trait_name in &proj.port_traits {
        let field = to_snake_case(trait_name);
        out.push_str(&format!("            {},\n", field));
    }
    out.push_str("        })\n");
    out.push_str("    }\n\n");

    // Convenience method: load_from_env
    out.push_str("    /// Load using the environment variable or default path.\n");
    out.push_str(
        "    pub fn load_from_env() -> Result<Self, Box<dyn std::error::Error>> {\n",
    );
    out.push_str(&format!(
        "        let path = std::env::var(\"{}_LIB_PATH\")\n",
        proj.env_name
    ));
    out.push_str(&format!(
        "            .unwrap_or_else(|_| \"./libs/lib{}.so\".to_string());\n",
        proj.slug
    ));
    out.push_str("        Self::load(&path)\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
}

/// Generate factory functions for a library project compiled as cdylib.
/// These are the `#[no_mangle] extern "C"` exports that the consumer's loader calls.
pub fn gen_cdylib_factory_functions(
    solution: &Solution,
    _registry: &LayerRegistry,
) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("//! Factory functions for shared library export.\n");
    out.push_str("//! These are the cdylib entry points loaded by consumers via `libloading`.\n\n");
    out.push_str("#![allow(unsafe_code)]\n\n");
    out.push_str("use crate::ports::*;\n");
    out.push_str("use crate::adapters::*;\n\n");

    // Find all adapter constructs (Shape::Impl) and their target traits
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            collect_adapter_factories(&mut out, c);
        }
    }

    out
}

/// Recursively find adapter (impl) constructs and generate factory functions.
fn collect_adapter_factories(out: &mut String, c: &Construct) {
    if c.shape == Shape::Impl {
        // The adapter's "for" clause names the port trait
        if let Some(target_trait) = &c.target {
            let factory_name = format!("create_{}", to_snake_case(target_trait));
            let adapter_name = &c.name;

            out.push_str(&format!(
                "/// Factory for `{}` → `dyn {}`.\n",
                adapter_name, target_trait
            ));
            out.push_str("#[no_mangle]\n");
            out.push_str(&format!(
                "pub unsafe extern \"C\" fn {}() -> *mut (dyn {} + Send + Sync) {{\n",
                factory_name, target_trait
            ));
            out.push_str(&format!(
                "    let adapter = {}::new();\n",
                adapter_name
            ));
            out.push_str(&format!(
                "    Box::into_raw(Box::new(adapter) as Box<dyn {} + Send + Sync>)\n",
                target_trait
            ));
            out.push_str("}\n\n");
        }
    }

    // Recurse into children (adapters might be nested under modules)
    for child in &c.children {
        collect_adapter_factories(out, child);
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Public accessor for to_snake_case (used by harness_template augmentation).
pub fn to_snake_case_pub(s: &str) -> String {
    to_snake_case(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("dlx_auth"), "DlxAuth");
        assert_eq!(to_pascal_case("session_store"), "SessionStore");
    }

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("JwksProvider"), "jwks_provider");
        assert_eq!(to_snake_case("SessionStore"), "session_store");
        assert_eq!(to_snake_case("CohortLookup"), "cohort_lookup");
    }

    #[test]
    fn test_gen_loader_struct() {
        let proj = LinkedProjectInfo {
            link: LinkDecl {
                name: "dlx_auth".to_string(),
                path: None,
                features: vec![],
                is_project_link: true,
                version: Some("2.1.0".to_string()),
                span: veil_ir::span::Span::new(0, 0),
            },
            loader_struct_name: "LinkedDlxAuth".to_string(),
            env_name: "DLX_AUTH".to_string(),
            slug: "dlx_auth".to_string(),
            port_traits: vec![
                "JwksProvider".to_string(),
                "SessionStore".to_string(),
            ],
        };

        let mut out = String::new();
        gen_loader_struct(&mut out, &proj);

        assert!(out.contains("pub struct LinkedDlxAuth"));
        assert!(out.contains("pub jwks_provider: Box<dyn JwksProvider + Send + Sync>"));
        assert!(out.contains("pub session_store: Box<dyn SessionStore + Send + Sync>"));
        assert!(out.contains("fn load(path: &str)"));
        assert!(out.contains("lib.get(b\"create_jwks_provider\")"));
        assert!(out.contains("lib.get(b\"create_session_store\")"));
        assert!(out.contains("DLX_AUTH_LIB_PATH"));
    }
}
