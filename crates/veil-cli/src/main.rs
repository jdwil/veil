//! VEIL CLI — parse, validate, generate, and serve VEIL files.
//!
//! All vocabulary comes from `.layer` files referenced by the input's `use`
//! lines. The CLI contains zero domain knowledge: it loads the layer registry,
//! hands it to the parser, and serves palette metadata straight from it.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use veil_ir::LayerRegistry;


#[derive(Parser)]
#[command(name = "veil", version, about = "VEIL — Visual Engineering Intermediate Language")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Lex a VEIL file and print tokens
    Lex {
        /// Path to the .veil file
        file: PathBuf,
    },
    /// Parse a VEIL file and print AST
    Parse {
        /// Path to the .veil file
        file: PathBuf,
    },
    /// Check a VEIL file (parse + structural validation + graph diagnostics).
    ///
    /// Exit code 1 if any **error**-severity diagnostic is present.
    /// Warnings are printed but do not fail the process.
    ///
    /// Severity:
    ///   error   — constraint violation; must fix
    ///   warning — advisory (e.g. port without impl)
    Check {
        /// Path to the .veil file
        file: PathBuf,
        /// Codegen target for capability checks (rust, typescript). Default: rust
        #[arg(short = 't', long, default_value = "rust")]
        target: String,
        /// Also emit multi-target debt warnings (features not honest on *other*
        /// targets). Off by default — only the selected `-t` target is
        /// capability-checked (primary-target only).
        #[arg(long)]
        target_debt: bool,
        /// Deprecated alias: multi-target debt is off by default (no-op).
        #[arg(long, hide = true)]
        no_target_debt: bool,
        /// Dump the IR graph as JSON after check
        #[arg(long)]
        dump_ir: bool,
        /// Execute and print layer codegen templates (debug)
        #[arg(long)]
        emit_templates: bool,
        /// Print the capability matrix for the selected target and exit
        #[arg(long)]
        list_capabilities: bool,
        /// Treat escape-hatch debt as errors (raw blocks, empty adapters, external calls, Json boundaries)
        #[arg(long)]
        deny_escape_hatches: bool,
        /// Print escape-hatch debt count summary (always on when any escape diags exist; force with this flag)
        #[arg(long)]
        escape_summary: bool,
        /// Emit machine-readable JSON report (PAR-010 metrics)
        #[arg(long)]
        json: bool,
    },
    /// Assemble layer prompts + construct outline for agents (PAR-009)
    Prompt {
        /// Path to the .veil file
        file: PathBuf,
        /// Max approximate tokens (chars/4); 0 = unlimited
        #[arg(long, default_value = "0")]
        max_tokens: usize,
    },
    /// Generate code from a VEIL file
    Gen {
        /// Path to the .veil file
        file: PathBuf,
        /// Output directory
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
        /// Target language (rust, typescript)
        #[arg(short, long, default_value = "rust")]
        target: String,
    },
    /// Start the visualization server for a **single project** root
    /// (packages + project layers). Multi-project hub is runtime UX —
    /// use `veil projects` and open IDE per product path.
    Serve {
        /// Path to a .veil file or project directory (omit with `--multi`)
        #[arg(required_unless_present = "multi")]
        file: Option<PathBuf>,
        /// Port to serve on
        #[arg(short, long, default_value = "3001")]
        port: u16,
        /// Multi-project hub: one process, `/api/p/{project}/…` (MP-002)
        #[arg(long)]
        multi: bool,
        /// Include core platform `.layer` files (ddd, base, …) in the IDE file list.
        /// Default: hide them (userland packages + family/client layers only).
        /// Core layers remain loadable via `use` for packages.
        /// Env: `VEIL_SHOW_CORE_LAYERS=1` is equivalent.
        #[arg(long, default_value_t = false)]
        show_core_layers: bool,
        /// Non-interactive first-run / empty-root prompts (also CI=1).
        #[arg(long, alias = "yes")]
        non_interactive: bool,
    },
    /// Scaffold a VEIL product project (`veil.toml`, package, layers/, stubs/)
    Init {
        /// Directory to initialize (default: `.`)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Product name (default: directory basename)
        #[arg(long)]
        name: Option<String>,
        /// Create under configured projects_dir/<name>
        #[arg(long)]
        in_hub: bool,
        /// Skip git init
        #[arg(long)]
        no_git: bool,
        /// Allow non-empty / re-scaffold
        #[arg(long)]
        force: bool,
        /// Non-interactive first-run
        #[arg(long, alias = "yes")]
        non_interactive: bool,
    },
    /// Manage the local projects directory (runtime hub; independent git repos).
    ///
    /// Config: `~/.veil/config.json` (first run prompts for projects_dir).
    /// Scaffold: `veil init` or `veil projects create`.
    Projects {
        #[command(subcommand)]
        action: ProjectsCmd,
        /// Non-interactive first-run
        #[arg(long, global = true, alias = "yes")]
        non_interactive: bool,
    },
    /// Serialize: parse then re-emit VEIL source (round-trip test)
    Emit {
        /// Path to the .veil file
        file: PathBuf,
    },
    /// Generate a .stub file from a Rust crate's rustdoc JSON
    StubGen {
        /// Crate name (e.g. "reqwest")
        crate_name: String,
        /// Output .stub file path (default: <crate_name>.stub)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Path to a Cargo project that has the crate as a dependency
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Subcommand)]
enum ProjectsCmd {
    /// Print the resolved projects directory (config / env)
    Dir {
        /// Update config `projects_dir` (does not move existing repos)
        #[arg(long)]
        set: Option<PathBuf>,
    },
    /// List product projects under the projects directory
    List,
    /// Create a new product under the projects hub (same as `veil init --in-hub --name`)
    Create {
        /// Project name ([a-zA-Z0-9_-]+)
        name: String,
        /// Skip git init
        #[arg(long)]
        no_git: bool,
    },
    /// Print absolute path to a named project
    Path {
        name: String,
    },
}

/// Load the layer registry for a .veil file, exiting on layer errors.
fn registry_for(file: &std::path::Path) -> LayerRegistry {
    match LayerRegistry::for_veil_file(file) {
        Ok(reg) => reg,
        Err(e) => {
            eprintln!("Layer error: {}", e);
            std::process::exit(1);
        }
    }
}

fn env_flag_true(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

/// UX-010 / DSL-002: whether a loaded file may be written via the IDE edit API.
///
/// Editable: `.veil` packages and `.layer` language files.
/// Read-only:
/// - other extensions (e.g. `.stub` MVP)
/// - path contains `generated/`
/// - first non-empty line is `# veil:readonly`
fn is_veil_source_editable(path: &std::path::Path, source: &str) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "veil" && ext != "layer" {
        return false;
    }
    // Match generated/ as a path component (absolute or relative).
    if path.components().any(|c| c.as_os_str() == "generated") {
        return false;
    }
    for line in source.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t == "# veil:readonly" || t.starts_with("# veil:readonly ") {
            return false;
        }
        break;
    }
    true
}

#[cfg(test)]
mod editable_tests {
    use super::is_veil_source_editable;
    use std::path::Path;

    #[test]
    fn pkg_files_are_editable() {
        assert!(is_veil_source_editable(
            Path::new("examples/hello_world.veil"),
            "pkg HelloWorld\n  use ddd\n"
        ));
    }

    #[test]
    fn sol_files_are_editable() {
        assert!(is_veil_source_editable(
            Path::new("app.veil"),
            "sol App\n  use ddd\n"
        ));
    }

    #[test]
    fn readonly_marker_and_generated_not_editable() {
        assert!(!is_veil_source_editable(
            Path::new("lock.veil"),
            "# veil:readonly\npkg Locked\n"
        ));
        assert!(!is_veil_source_editable(
            Path::new("generated/out.veil"),
            "pkg X\n"
        ));
    }

    #[test]
    fn layer_files_are_editable() {
        assert!(is_veil_source_editable(
            Path::new("layers/ddd.layer"),
            "pkg ddd v1\n  construct X\n"
        ));
    }
}

fn parse_solution_or_exit(source: &str, file: &std::path::Path) -> (veil_ir::Solution, LayerRegistry) {
    let tokens = veil_parser::lex(source);
    let registry = registry_for(file);
    // Prefer full file parse so package `adapt` / patches are preserved for merge.
    let veil_file = match veil_parser::parse_file_with_registry(&tokens, registry.clone()) {
        Ok(f) => f,
        Err(errors) => {
            eprintln!("Parse errors:");
            for err in &errors {
                eprintln!("  {}", err);
            }
            std::process::exit(1);
        }
    };
    match veil_file {
        veil_ir::VeilFile::Package(pkg)
            if !pkg.adapts.is_empty() || !pkg.patches.is_empty() =>
        {
            match merge_package_or_exit(&pkg, file) {
                Ok(sol) => (sol, registry),
                Err(()) => std::process::exit(1),
            }
        }
        veil_ir::VeilFile::Package(_pkg) => {
            // Lower package → solution + inject declarations (same as parse_with_registry).
            match veil_parser::parse_with_registry(&tokens, registry.clone()) {
                Ok(sol) => (sol, registry),
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            }
        }
        veil_ir::VeilFile::Solution(_) | veil_ir::VeilFile::Composition(_) => {
            match veil_parser::parse_with_registry(&tokens, registry.clone()) {
                Ok(sol) => (sol, registry),
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Load adapt bases and flatten to a Solution for check/codegen.
fn merge_package_or_exit(
    leaf: &veil_ir::Package,
    leaf_path: &std::path::Path,
) -> Result<veil_ir::Solution, ()> {
    let search = veil_ir::default_adapt_search_paths(leaf_path, &[]);
    let load = |name: &str| -> Result<veil_ir::Package, String> {
        let path = veil_ir::find_package_source(name, &search)
            .ok_or_else(|| format!("not found in {:?}", search))?;
        let src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let tokens = veil_parser::lex(&src);
        let reg = veil_ir::LayerRegistry::for_veil_file(&path)
            .unwrap_or_else(|_| veil_ir::LayerRegistry::builtin());
        match veil_parser::parse_file_with_registry(&tokens, reg) {
            Ok(veil_ir::VeilFile::Package(p)) => Ok(p),
            Ok(_) => Err(format!("'{name}' is not a package")),
            Err(errs) => Err(errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")),
        }
    };
    match veil_ir::merge_adapted_package(leaf, load) {
        Ok(merged) => {
            if !merged.chain.is_empty() && merged.chain.len() > 1 {
                eprintln!(
                    "adapt chain: {}",
                    merged.chain.join(" → ")
                );
            }
            // Re-inject layer declarations on flattened IR via serialize → parse.
            let registry = registry_for(leaf_path);
            let emitted = veil_ir::serialize_package(&merged.package);
            let tokens = veil_parser::lex(&emitted);
            match veil_parser::parse_with_registry(&tokens, registry) {
                Ok(s) => Ok(s),
                Err(_) => Ok(veil_ir::package_as_solution(&merged.package)),
            }
        }
        Err(e) => {
            eprintln!("adapt error [{}]: {}", e.code, e.message);
            Err(())
        }
    }
}


/// Generate a .stub file by running `cargo +nightly rustdoc --output-format json`
/// and converting the JSON into VEIL stub format.
/// Creates a temporary Cargo project with the crate as a dependency if no project is specified.
fn generate_stub(crate_name: &str, project_dir: &std::path::Path) -> Result<String, String> {
    use std::process::Command;

    // If project_dir is "." and doesn't have a Cargo.toml with this crate,
    // create a temporary project
    let (work_dir, _temp_dir) = if project_dir == std::path::Path::new(".") || !project_dir.join("Cargo.toml").exists() {
        let tmp = tempfile::tempdir().map_err(|e| format!("Cannot create temp dir: {}", e))?;
        let tmp_path = tmp.path().to_path_buf();

        // Create a minimal Cargo project
        let init = Command::new("cargo")
            .args(["init", "--lib", "--name", "stub-workspace"])
            .current_dir(&tmp_path)
            .output()
            .map_err(|e| format!("Failed to init temp project: {}", e))?;
        if !init.status.success() {
            return Err(format!("cargo init failed: {}", String::from_utf8_lossy(&init.stderr)));
        }

        // Add the target crate as a dependency
        let add = Command::new("cargo")
            .args(["add", crate_name])
            .current_dir(&tmp_path)
            .output()
            .map_err(|e| format!("Failed to add dep: {}", e))?;
        if !add.status.success() {
            return Err(format!("cargo add {} failed: {}", crate_name, String::from_utf8_lossy(&add.stderr)));
        }

        eprintln!("  Creating temp project with {} as dependency...", crate_name);
        (tmp_path, Some(tmp))
    } else {
        (project_dir.to_path_buf(), None)
    };

    eprintln!("  Running cargo +nightly rustdoc...");

    // Run cargo rustdoc to generate JSON
    let output = Command::new("cargo")
        .args(["+nightly", "rustdoc", "-p", crate_name, "--", "--output-format", "json", "-Z", "unstable-options"])
        .current_dir(&work_dir)
        .output()
        .map_err(|e| format!("Failed to run cargo: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo rustdoc failed: {}", stderr));
    }

    // Find the generated JSON file
    let crate_file_name = crate_name.replace('-', "_");
    let json_path = work_dir.join("target").join("doc").join(format!("{}.json", crate_file_name));
    if !json_path.exists() {
        // Try original name
        let alt_path = work_dir.join("target").join("doc").join(format!("{}.json", crate_name));
        if !alt_path.exists() {
            return Err(format!("JSON file not found at {:?}", json_path));
        }
        let content = std::fs::read_to_string(&alt_path)
            .map_err(|e| format!("Cannot read JSON: {}", e))?;
        return convert_rustdoc_json_to_stub(&content, crate_name);
    }

    let content = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("Cannot read JSON: {}", e))?;
    convert_rustdoc_json_to_stub(&content, crate_name)
}

/// Convert rustdoc JSON to .stub file format.
fn convert_rustdoc_json_to_stub(json_str: &str, crate_name: &str) -> Result<String, String> {
    let data: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let version = data.get("crate_version")
        .and_then(|v| v.as_str())
        .unwrap_or("*");

    let index = data.get("index")
        .and_then(|v| v.as_object())
        .ok_or("No index in JSON")?;

    let mut out = format!("stub {} {}\n", crate_name, version);

    // Collect structs and their impl items
    let mut struct_ids: Vec<(String, String)> = Vec::new(); // (id, name)
    let mut struct_impls: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new(); // name → method signatures

    for (id, item) in index {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let vis = item.get("visibility").and_then(|v| v.as_str()).unwrap_or("");
        if vis != "public" { continue; }

        let inner = match item.get("inner").and_then(|v| v.as_object()) {
            Some(i) => i,
            None => continue,
        };

        // Collect public structs
        if inner.contains_key("struct") {
            struct_ids.push((id.clone(), name.to_string()));
            // Get impls for this struct
            if let Some(struct_data) = inner.get("struct").and_then(|v| v.as_object()) {
                if let Some(impls) = struct_data.get("impls").and_then(|v| v.as_array()) {
                    for impl_id in impls {
                        let impl_id_str = impl_id.as_u64().map(|n| n.to_string())
                            .or_else(|| impl_id.as_str().map(|s| s.to_string()));
                        if let Some(impl_id_str) = impl_id_str {
                            if let Some(impl_item) = index.get(&impl_id_str) {
                                if let Some(impl_inner) = impl_item.get("inner")
                                    .and_then(|v| v.as_object())
                                    .and_then(|o| o.get("impl"))
                                    .and_then(|v| v.as_object())
                                {
                                    // Skip trait impls (Display, Clone, etc.)
                                    if impl_inner.get("trait_").is_some() { continue; }
                                    if let Some(items) = impl_inner.get("items").and_then(|v| v.as_array()) {
                                        for method_id in items {
                                            let method_id_str = method_id.as_u64().map(|n| n.to_string())
                                                .or_else(|| method_id.as_str().map(|s| s.to_string()));
                                            if let Some(mid) = method_id_str {
                                                if let Some(method) = index.get(&mid) {
                                                    let method_vis = method.get("visibility").and_then(|v| v.as_str()).unwrap_or("");
                                                    if method_vis != "public" { continue; }
                                                    if let Some(sig) = extract_method_sig(method) {
                                                        struct_impls.entry(name.to_string())
                                                            .or_default()
                                                            .push(sig);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Emit structs with their methods
    for (_, name) in &struct_ids {
        out.push_str(&format!("\n  struct {}\n", name));
        if let Some(methods) = struct_impls.get(name) {
            for sig in methods {
                out.push_str(&format!("    {}\n", sig));
            }
        }
    }

    // Collect and emit public traits with their methods
    for (_id, item) in index {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let vis = item.get("visibility").and_then(|v| v.as_str()).unwrap_or("");
        if vis != "public" { continue; }

        let inner = match item.get("inner").and_then(|v| v.as_object()) {
            Some(i) => i,
            None => continue,
        };

        if let Some(trait_data) = inner.get("trait").and_then(|v| v.as_object()) {
            let items = trait_data.get("items").and_then(|v| v.as_array());
            let mut methods = Vec::new();
            if let Some(items) = items {
                for method_id in items {
                    let method_id_str = method_id.as_u64().map(|n| n.to_string())
                        .or_else(|| method_id.as_str().map(|s| s.to_string()));
                    if let Some(mid) = method_id_str {
                        if let Some(method) = index.get(&mid) {
                            // Only include function items (skip associated types, consts)
                            if let Some(method_inner) = method.get("inner").and_then(|v| v.as_object()) {
                                if method_inner.contains_key("function") {
                                    if let Some(sig) = extract_method_sig(method) {
                                        methods.push(sig);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !methods.is_empty() {
                out.push_str(&format!("\n  trait {}\n", name));
                for sig in &methods {
                    out.push_str(&format!("    {}\n", sig));
                }
            }
        }
    }

    Ok(out)
}

/// Extract a method signature from a rustdoc JSON item.
fn extract_method_sig(item: &serde_json::Value) -> Option<String> {
    let name = item.get("name")?.as_str()?;
    let inner = item.get("inner")?.as_object()?;
    let func = inner.get("function")?.as_object()?;
    let sig = func.get("sig")?.as_object()?;

    // Build params
    let inputs = sig.get("inputs").and_then(|v| v.as_array())?;
    let params: Vec<String> = inputs.iter().filter_map(|input| {
        let arr = input.as_array()?;
        let param_name = arr.get(0)?.as_str()?;
        if param_name == "self" { return None; } // Skip self
        let type_val = arr.get(1)?;
        let type_str = rustdoc_type_to_veil(type_val);
        Some(format!("{}: {}", param_name, type_str))
    }).collect();

    // Build return type
    let output = sig.get("output");
    let ret = output.and_then(|o| {
        if o.is_null() { return None; }
        let t = rustdoc_type_to_veil(o);
        if t == "()" { None } else { Some(t) }
    });

    let sig_str = if let Some(ret) = ret {
        format!("fn {}({}) -> {}", name, params.join(", "), ret)
    } else {
        format!("fn {}({})", name, params.join(", "))
    };
    Some(sig_str)
}

/// Convert a rustdoc JSON type representation to VEIL type syntax.
fn rustdoc_type_to_veil(ty: &serde_json::Value) -> String {
    if let Some(obj) = ty.as_object() {
        if let Some(path) = obj.get("resolved_path").and_then(|v| v.as_object()) {
            let type_path = path.get("path").and_then(|v| v.as_str()).unwrap_or("Unknown");
            // Simplify: take the last segment
            let simple = type_path.rsplit("::").next().unwrap_or(type_path);
            // Map common Rust types to VEIL types
            let veil_type = match simple {
                "String" | "str" => "Str",
                "bool" => "Bool",
                "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "usize" | "isize" => "Int",
                "f32" | "f64" => "F64",
                "Vec" => "List",
                "Option" => "Opt",
                "Result" => "Res",
                other => other,
            };
            // Handle generics
            if let Some(args) = path.get("args").and_then(|v| v.as_object()) {
                if let Some(angle) = args.get("angle_bracketed").and_then(|v| v.as_object()) {
                    if let Some(type_args) = angle.get("args").and_then(|v| v.as_array()) {
                        let arg_strs: Vec<String> = type_args.iter().filter_map(|a| {
                            a.as_object()?.get("type").map(|t| rustdoc_type_to_veil(t))
                        }).collect();
                        if !arg_strs.is_empty() {
                            if veil_type == "Res" {
                                return format!("Res!<{}>", arg_strs.first().unwrap_or(&"()".to_string()));
                            }
                            return format!("{}<{}>", veil_type, arg_strs.join(", "));
                        }
                    }
                }
            }
            return veil_type.to_string();
        }
        if let Some(prim) = obj.get("primitive").and_then(|v| v.as_str()) {
            return match prim {
                "bool" => "Bool".to_string(),
                "str" => "Str".to_string(),
                "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" => "Int".to_string(),
                "f32" | "f64" => "F64".to_string(),
                other => other.to_string(),
            };
        }
        if let Some(g) = obj.get("generic").and_then(|v| v.as_str()) {
            return g.to_string();
        }
        if let Some(borrow) = obj.get("borrowed_ref").and_then(|v| v.as_object()) {
            if let Some(inner) = borrow.get("type") {
                return rustdoc_type_to_veil(inner);
            }
        }
    }
    "Str".to_string() // fallback
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("veil=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Prompt { file, max_tokens } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let (sol, registry) = parse_solution_or_exit(&source, &file);
            let mut parts: Vec<String> = Vec::new();
            // Layer prompts in load order (PAR-009)
            for (layer, text) in &registry.prompts {
                parts.push(format!("# Layer prompt: {layer}\n{text}"));
            }
            // Compact construct outline
            let graph = veil_ir::build_ir_with_registry(&sol, Some(&registry));
            let mut outline = vec![format!("# Package {}\nconstructs:", sol.name)];
            for n in &graph.nodes {
                if matches!(
                    n.kind,
                    veil_ir::NodeKind::Solution
                        | veil_ir::NodeKind::Action
                        | veil_ir::NodeKind::Field
                        | veil_ir::NodeKind::Inputs
                        | veil_ir::NodeKind::Return
                ) {
                    continue;
                }
                let sk = n.metadata.subkind.as_deref().unwrap_or("");
                outline.push(format!("- {:?} {} {}", n.kind, sk, n.name));
            }
            parts.push(outline.join("\n"));
            // Constraints from palette (layer vocabulary)
            let palette = veil_ir::palette_from_registry(&registry);
            let mut vocab = vec!["# Vocabulary (keywords):".to_string()];
            for e in palette.iter().take(80) {
                vocab.push(format!("- {} ({})", e.keyword, e.name));
            }
            parts.push(vocab.join("\n"));

            let mut text = parts.join("\n\n---\n\n");
            if max_tokens > 0 {
                let max_chars = max_tokens.saturating_mul(4);
                if text.len() > max_chars {
                    text.truncate(max_chars);
                    text.push_str("\n\n…[truncated for token budget]…\n");
                }
            }
            println!("{text}");
        }
        Commands::Lex { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            for token in &tokens {
                println!("{:?}", token);
            }
        }
        Commands::Parse { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let (sol, _) = parse_solution_or_exit(&source, &file);
            println!("{}", serde_json::to_string_pretty(&sol).unwrap());
        }
        Commands::Check {
            file,
            target,
            target_debt,
            no_target_debt: _no_target_debt,
            dump_ir,
            emit_templates,
            list_capabilities,
            deny_escape_hatches,
            escape_summary,
            json,
        } => {
            let codegen_target = veil_codegen::CodegenTarget::from_str(&target).unwrap_or_else(|| {
                eprintln!(
                    "Unknown target '{}'. Use: rust, typescript (rs, ts)",
                    target
                );
                std::process::exit(2);
            });

            if list_capabilities {
                println!("{}", veil_codegen::target_capability_summary(codegen_target));
                return;
            }

            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let file_display = file.display().to_string();

            // DSL-003: layer files use layer check pipeline
            if file.extension().and_then(|e| e.to_str()) == Some("layer") {
                let name = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "layer".into());
                let started = std::time::Instant::now();
                let mut diagnostics = veil_ir::check_layer(&source, &name);
                veil_ir::sort_diagnostics(&mut diagnostics);
                let duration_ms = started.elapsed().as_millis();
                let errors = diagnostics
                    .iter()
                    .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
                    .count();
                let warnings = diagnostics
                    .iter()
                    .filter(|d| matches!(d.severity, veil_ir::Severity::Warning))
                    .count();
                for d in &diagnostics {
                    println!(
                        "{}: {}",
                        file_display,
                        veil_ir::format_diagnostic_line(d)
                    );
                }
                println!(
                    "{} error(s), {} warning(s) ({} ms) [layer]",
                    errors, warnings, duration_ms
                );
                if errors > 0 {
                    std::process::exit(1);
                }
                return;
            }

            let (sol, registry) = parse_solution_or_exit(&source, &file);

            let started = std::time::Instant::now();
            let mut result = veil_ir::check_solution(&sol, &registry);

            // Target capability matrix (CHK-005) — selected target only
            result.diagnostics.extend(veil_codegen::check_target_capabilities(
                &sol,
                &registry,
                codegen_target,
            ));
            // Optional multi-target debt (opt-in): warn about other targets' gaps
            if target_debt && codegen_target == veil_codegen::CodegenTarget::Rust {
                result
                    .diagnostics
                    .extend(veil_codegen::check_multi_target_debt(&sol, &registry));
            }

            // Escape-hatch debt (CHK-006): already included in check_solution as warnings
            if deny_escape_hatches {
                veil_ir::promote_escape_hatches(&mut result.diagnostics);
            }

            veil_ir::sort_diagnostics(&mut result.diagnostics);
            let duration_ms = started.elapsed().as_millis();

            let hatch_counts =
                veil_ir::EscapeHatchSummary::from_diagnostics(&result.diagnostics);

            if json {
                // PAR-010: machine-readable metrics for CI dashboards
                let report = serde_json::json!({
                    "file": file_display,
                    "package": sol.name,
                    "target": target,
                    "layers": registry.layers,
                    "ok": !result.has_errors(),
                    "error_count": result.error_count(),
                    "warning_count": result.warning_count(),
                    "node_count": result.graph.nodes.len(),
                    "edge_count": result.graph.edges.len(),
                    "duration_ms": duration_ms,
                    "escape_hatch": {
                        "raw_surface": hatch_counts.raw_surface,
                        "empty_adapter": hatch_counts.empty_adapter,
                        "external_call": hatch_counts.external_call,
                        "json_boundary": hatch_counts.json_boundary,
                        "total": hatch_counts.total(),
                    },
                    "diagnostics": result.diagnostics.iter().map(|d| serde_json::json!({
                        "severity": format!("{:?}", d.severity),
                        "code": d.code,
                        "message": d.message,
                        "node_name": d.node_name,
                    })).collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
                if result.has_errors() {
                    std::process::exit(1);
                }
                return;
            }

            // Compact summary header
            eprintln!(
                "check {} — {} (layers: {}; target: {})",
                file_display,
                sol.name,
                if registry.layers.is_empty() {
                    "—".to_string()
                } else {
                    registry.layers.join(", ")
                },
                target
            );

            if emit_templates && !registry.codegen_templates.is_empty() {
                let tpl_target = match codegen_target {
                    veil_codegen::CodegenTarget::Rust => "rust",
                    veil_codegen::CodegenTarget::TypeScript => "typescript",
                    veil_codegen::CodegenTarget::Swift => "swift",
                    veil_codegen::CodegenTarget::Kotlin => "kotlin",
                };
                let output = veil_codegen::execute_templates(&sol, &registry, tpl_target);
                for f in &output.files {
                    println!("template-file {}: {} bytes", f.path, f.content.len());
                    println!("{}", f.content);
                }
                for (name, contributions) in &output.sections {
                    for c in contributions {
                        println!(
                            "template-section {} priority={} from={}\n{}",
                            name, c.priority, c.source_layer, c.content
                        );
                    }
                }
            }

            if result.diagnostics.is_empty() {
                eprintln!(
                    "ok — {} node(s), {} edge(s), 0 diagnostics ({} ms)",
                    result.graph.nodes.len(),
                    result.graph.edges.len(),
                    duration_ms
                );
            } else {
                for d in &result.diagnostics {
                    // stdout for machine-friendly piping; include file path
                    println!("{}: {}", file_display, veil_ir::format_diagnostic_line(d));
                }
                eprintln!(
                    "{} error(s), {} warning(s) ({} ms)",
                    result.error_count(),
                    result.warning_count(),
                    duration_ms
                );
            }

            // Metric-friendly escape-hatch summary (CHK-006)
            if hatch_counts.total() > 0 || escape_summary {
                eprintln!("{}", hatch_counts.format_line());
            }

            if dump_ir {
                println!("{}", serde_json::to_string_pretty(&result.graph).unwrap());
            }

            if result.has_errors() {
                std::process::exit(1);
            }
        }
        Commands::Gen { file, output, target } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let registry = registry_for(&file);
            let tokens = veil_parser::lex(&source);
            let veil_file = match veil_parser::parse_file_with_registry(&tokens, registry.clone()) {
                Ok(f) => f,
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors { eprintln!("  {}", err); }
                    std::process::exit(1);
                }
            };

            let codegen_target = veil_codegen::CodegenTarget::from_str(&target)
                .unwrap_or_else(|| {
                    eprintln!(
                        "Unknown target '{}'. Use: rust, typescript, swift, kotlin",
                        target
                    );
                    std::process::exit(1);
                });

            let files = match &veil_file {
                veil_ir::ast::VeilFile::Solution(sol) => {
                    // For TypeScript target: check if any used packages have expose blocks
                    if codegen_target == veil_codegen::CodegenTarget::TypeScript {
                        let dir = file.parent().unwrap_or(std::path::Path::new("."));
                        let mut used_pkgs: Vec<(String, veil_ir::ast::ExposeBlock)> = Vec::new();
                        for use_ref in &sol.uses {
                            // Try loading the package file to find its expose block
                            let pkg_path = dir.join(format!("{}.veil", use_ref.package_name));
                            if pkg_path.exists() {
                                if let Ok(pkg_source) = std::fs::read_to_string(&pkg_path) {
                                    let pkg_tokens = veil_parser::lex(&pkg_source);
                                    let pkg_reg = registry_for(&pkg_path);
                                    if let Ok(veil_ir::ast::VeilFile::Package(pkg)) =
                                        veil_parser::parse_file_with_registry(&pkg_tokens, pkg_reg) {
                                        if let Some(expose) = pkg.expose {
                                            used_pkgs.push((pkg.name, expose));
                                        }
                                    }
                                }
                            }
                        }
                        veil_codegen::generate_for_target_with_packages(sol, &registry, codegen_target, &used_pkgs)
                    } else {
                        veil_codegen::generate_for_target(sol, &registry, codegen_target)
                    }
                }
                veil_ir::ast::VeilFile::Package(pkg) => {
                    // ADP-010/011: adapt chain → flatten before codegen.
                    let sol = if !pkg.adapts.is_empty() || !pkg.patches.is_empty() {
                        match merge_package_or_exit(pkg, &file) {
                            Ok(s) => s,
                            Err(()) => std::process::exit(1),
                        }
                    } else {
                        veil_ir::ast::Solution {
                            name: pkg.name.clone(),
                            span: pkg.span,
                            uses: pkg.uses.clone(),
                            links: pkg.links.clone(),
                            items: pkg.items.clone(),
                            expose: pkg.expose.clone(),
                        }
                    };
                    match codegen_target {
                        veil_codegen::CodegenTarget::TypeScript => {
                            // Prefer full module/SPA gen; if package only has expose and
                            // no constructs, fall back to API client.
                            let has_constructs = sol.items.iter().any(|i| {
                                matches!(i, veil_ir::ast::TopLevelItem::Construct(_))
                            });
                            if has_constructs {
                                veil_codegen::generate_for_target(&sol, &registry, codegen_target)
                            } else if sol.expose.is_some() {
                                // API client from pre-merge package expose when no constructs
                                let project =
                                    veil_codegen::typescript::generate_api_client_from_package(pkg);
                                project
                                    .files
                                    .into_iter()
                                    .map(|f| veil_codegen::GeneratedFile {
                                        path: f.path,
                                        content: f.content,
                                    })
                                    .collect()
                            } else {
                                veil_codegen::generate_for_target(&sol, &registry, codegen_target)
                            }
                        }
                        veil_codegen::CodegenTarget::Rust
                        | veil_codegen::CodegenTarget::Swift
                        | veil_codegen::CodegenTarget::Kotlin => {
                            veil_codegen::generate_for_target(&sol, &registry, codegen_target)
                        }
                    }
                }
                _ => {
                    eprintln!("Composition files do not generate code directly.");
                    std::process::exit(1);
                }
            };

            for f in &files {
                let path = output.join(&f.path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("Failed to create directory");
                }
                std::fs::write(&path, &f.content).expect("Failed to write file");
            }

            // Run formatters on generated files (spikes skip formatters)
            match codegen_target {
                veil_codegen::CodegenTarget::Rust => {
                    for f in &files {
                        if f.path.ends_with(".rs") {
                            let path = output.join(&f.path);
                            let _ = std::process::Command::new("rustfmt")
                                .args(["--edition", "2024", &path.to_string_lossy()])
                                .output();
                        }
                    }
                }
                veil_codegen::CodegenTarget::TypeScript => {
                    // Optionally run prettier if available
                    let _ = std::process::Command::new("npx")
                        .args(["prettier", "--write", &output.to_string_lossy()])
                        .output();
                }
                veil_codegen::CodegenTarget::Swift | veil_codegen::CodegenTarget::Kotlin => {}
            }

            println!(
                "✓ Generated {} files ({}) in {}",
                files.len(),
                target,
                output.display()
            );
        }
        Commands::Emit { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let (sol, _) = parse_solution_or_exit(&source, &file);
            let output = veil_ir::serialize_solution(&sol);
            print!("{}", output);
        }
        Commands::StubGen { crate_name, output, project } => {
            let output_path = output.unwrap_or_else(|| PathBuf::from(format!("{}.stub", crate_name)));
            match generate_stub(&crate_name, &project) {
                Ok(content) => {
                    std::fs::write(&output_path, &content)
                        .expect("Failed to write .stub file");
                    println!("✓ Generated {} ({} bytes)", output_path.display(), content.len());
                }
                Err(e) => {
                    eprintln!("Error generating stub: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Projects {
            action,
            non_interactive,
        } => {
            if let Err(e) = veil_server::ensure_config(non_interactive) {
                eprintln!("Config error: {e}");
                std::process::exit(1);
            }
            let _ = veil_server::ensure_projects_dir_exists();
            let dir = veil_server::default_projects_dir();
            match action {
                ProjectsCmd::Dir { set } => {
                    if let Some(path) = set {
                        match veil_server::set_projects_dir(&path) {
                            Ok(cfg) => {
                                println!("{}", cfg.projects_dir_path().display());
                                eprintln!(
                                    "✓ Updated {} (projects_dir; existing repos were not moved)",
                                    veil_server::config_path().display()
                                );
                            }
                            Err(e) => {
                                eprintln!("Error: {e}");
                                std::process::exit(1);
                            }
                        }
                    } else {
                        let d = veil_server::ensure_projects_dir_exists()
                            .unwrap_or_else(|_| dir.clone());
                        println!("{}", d.display());
                        eprintln!("# config: {}", veil_server::config_path().display());
                    }
                }
                ProjectsCmd::List => match veil_server::list_projects(&dir) {
                    Ok(projects) => {
                        println!("Projects directory: {}", dir.display());
                        println!("Config: {}", veil_server::config_path().display());
                        if projects.is_empty() {
                            println!("  (empty — create with: veil projects create <name>)");
                            println!("  (or: veil init --in-hub --name <name>)");
                        } else {
                            for p in projects {
                                let git = if p.is_git { "git" } else { "no-git" };
                                println!(
                                    "  {}  {}  ({} package(s), {git})",
                                    p.name, p.path, p.package_count
                                );
                            }
                        }
                        println!();
                        println!("IDE (single-project convenience):");
                        println!("  veil serve <path> -p 3001");
                        println!("  make serve PROJECT=<path>");
                        println!("Runtime: docs/IDE_RUNTIME.md");
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                },
                ProjectsCmd::Create { name, no_git } => {
                    match veil_server::create_project_with_opts(&dir, &name, !no_git) {
                        Ok(info) => {
                            println!("✓ Created project {} (hub; same as veil init --in-hub)", info.name);
                            println!("  path: {}", info.path);
                            println!("  git:  {}", if info.is_git { "yes" } else { "no" });
                            println!();
                            println!("Open IDE:");
                            println!("  veil serve {} -p 3001", info.path);
                        }
                        Err(e) => {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                ProjectsCmd::Path { name } => {
                    let p = dir.join(&name);
                    if !p.is_dir() {
                        eprintln!("Project not found: {}", p.display());
                        std::process::exit(1);
                    }
                    println!("{}", p.display());
                }
            }
        }
        Commands::Init {
            path,
            name,
            in_hub,
            no_git,
            force,
            non_interactive,
        } => {
            if let Err(e) = veil_server::ensure_config(non_interactive) {
                eprintln!("Config error: {e}");
                std::process::exit(1);
            }
            let _ = veil_server::ensure_projects_dir_exists();

            let (root, proj_name) = if in_hub {
                let n = name.unwrap_or_else(|| {
                    path.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .filter(|s| s != "." && !s.is_empty())
                        .unwrap_or_else(|| "app".into())
                });
                let hub = veil_server::default_projects_dir();
                (hub.join(&n), n)
            } else {
                let root = if path.as_os_str() == "." {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                } else {
                    path
                };
                let n = name.unwrap_or_else(|| {
                    root.file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "app".into())
                });
                (root, n)
            };

            let opts = veil_server::InitOptions {
                name: proj_name,
                git: !no_git,
                force,
            };
            match veil_server::init_project(&root, &opts) {
                Ok(info) => {
                    println!("✓ Initialized VEIL project {}", info.name);
                    println!("  path: {}", info.path);
                    println!("  git:  {}", if info.is_git { "yes" } else { "no" });
                    println!();
                    println!("Next:");
                    println!("  veil serve {} -p 3001", info.path);
                    println!("  veil check {}/{}.veil", info.path, info.name);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Serve {
            file,
            port,
            multi,
            show_core_layers,
            non_interactive,
        } => {
            // Durable prefs live in ~/.veil/config.json; first run may prompt.
            let _cfg = match veil_server::ensure_config(non_interactive) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Config warning: {e} — continuing with defaults");
                    veil_server::load_config_or_default()
                }
            };
            let projects_dir = veil_server::ensure_projects_dir_exists()
                .unwrap_or_else(|_| veil_server::default_projects_dir());
            let show_core_layers = show_core_layers
                || env_flag_true("VEIL_SHOW_CORE_LAYERS")
                || _cfg.show_core_layers;

            if multi {
                println!(
                    "✓ Multi-project hub on port {port}"
                );
                println!("  Projects dir: {}", projects_dir.display());
                println!("  Config:       {}", veil_server::config_path().display());
                println!("  Hub:          http://localhost:{port}/api/projects");
                println!("  IDE:          http://localhost:{port}/api/p/{{name}}/ir");
                let hub = veil_server::ProjectsHub::new(projects_dir, show_core_layers);
                let app = veil_server::build_multi_router(hub);
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async move {
                    let listener = bind_serve_port(port).await;
                    println!("  Listening on port {port} (multi-project)");
                    if let Err(e) = axum::serve(listener, app).await {
                        eprintln!("Server error: {e}");
                        std::process::exit(1);
                    }
                });
                return;
            }

            let file = file.expect("file required without --multi");
            // INIT-003: serve root must exist; ensure shape; empty → offer init
            if !file.exists() {
                eprintln!("error: path does not exist: {}", file.display());
                eprintln!("  veil init <dir>   or   veil projects create <name>");
                std::process::exit(1);
            }

            let project_root = if file.is_dir() {
                file.clone()
            } else {
                file.parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .to_path_buf()
            };

            if file.is_dir() {
                if let Err(e) = veil_server::ensure_project_shape(&file) {
                    eprintln!("warning: {e}");
                }
                if !veil_server::has_package_sources(&file) {
                    let non_int = non_interactive || veil_server::is_noninteractive(false);
                    if non_int {
                        eprintln!(
                            "error: no .veil packages in {} — run: veil init {}",
                            file.display(),
                            file.display()
                        );
                        std::process::exit(1);
                    }
                    eprint!(
                        "No packages in {}. Run scaffold here? [y/N] ",
                        file.display()
                    );
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    if line.trim().eq_ignore_ascii_case("y")
                        || line.trim().eq_ignore_ascii_case("yes")
                    {
                        let n = file
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "app".into());
                        if let Err(e) = veil_server::init_project(
                            &file,
                            &veil_server::InitOptions {
                                name: n,
                                git: true,
                                force: true,
                            },
                        ) {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    } else {
                        eprintln!("Aborted. Hint: veil init {}", file.display());
                        std::process::exit(1);
                    }
                }
            }

            // Single-project scan: packages + project layers only (no monorepo layers/)
            let mut project_files: Vec<PathBuf> = if file.is_dir() {
                match veil_server::collect_project_files(&file, show_core_layers) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("{e}");
                        eprintln!(
                            "Hint: projects hub is {}.",
                            veil_server::default_projects_dir().display()
                        );
                        eprintln!("  veil projects list");
                        eprintln!("  veil init --in-hub --name <name>");
                        std::process::exit(1);
                    }
                }
            } else {
                vec![file.clone()]
            };

            // Prefer a package as the initial active file when present
            if let Some(pkg_idx) = project_files.iter().position(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("veil")
            }) {
                if pkg_idx != 0 {
                    project_files.swap(0, pkg_idx);
                }
            }

            let first_file = &project_files[0];
            let first_source = std::fs::read_to_string(first_file).expect("Failed to read file");
            let registry = registry_for(first_file);

            // UX-010 / DSL-002: .veil and .layer editable unless readonly marker
            let file_entries: Vec<(PathBuf, String, bool)> = project_files
                .iter()
                .map(|path| {
                    let source = std::fs::read_to_string(path).expect("Failed to read file");
                    let editable = is_veil_source_editable(path, &source);
                    (path.clone(), source, editable)
                })
                .collect();

            let file_count = file_entries.len();
            let package_count = file_entries
                .iter()
                .filter(|(p, _, _)| p.extension().and_then(|e| e.to_str()) == Some("veil"))
                .count();
            let layer_count = file_count - package_count;

            let (node_count, edge_count) =
                if first_file.extension().and_then(|e| e.to_str()) == Some("layer") {
                    let name = first_file
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "layer".into());
                    match veil_ir::build_layer_ir(&first_source, &name) {
                        Ok(g) => (g.nodes.len(), g.edges.len()),
                        Err(_) => (0, 0),
                    }
                } else {
                    let tokens = veil_parser::lex(&first_source);
                    let veil_file =
                        match veil_parser::parse_file_with_registry(&tokens, registry.clone()) {
                            Ok(f) => f,
                            Err(errors) => {
                                eprintln!("Parse errors:");
                                for err in &errors {
                                    eprintln!("  {}", err);
                                }
                                std::process::exit(1);
                            }
                        };

                    let graph = match &veil_file {
                        veil_ir::VeilFile::Solution(sol) => {
                            veil_ir::build_ir_with_registry(sol, Some(&registry))
                        }
                        veil_ir::VeilFile::Package(pkg) => {
                            let sol = veil_ir::Solution {
                                name: pkg.name.clone(),
                                span: pkg.span,
                                uses: pkg.uses.clone(),
                                links: pkg.links.clone(),
                                items: pkg.items.clone(),
                                expose: pkg.expose.clone(),
                            };
                            veil_ir::build_ir_with_registry(&sol, Some(&registry))
                        }
                        veil_ir::VeilFile::Composition(comp) => {
                            let search_dir = first_file
                                .parent()
                                .unwrap_or(std::path::Path::new("."))
                                .to_path_buf();
                            let search_paths = vec![search_dir];
                            let mut resolved = Vec::new();
                            let found = veil_ir::find_package_files(&comp.imports, &search_paths);
                            for result in found {
                                match result {
                                    Ok((imp, path)) => {
                                        let pkg_source = std::fs::read_to_string(&path)
                                            .expect("Failed to read package");
                                        let pkg_tokens = veil_parser::lex(&pkg_source);
                                        let pkg_registry =
                                            veil_ir::LayerRegistry::for_veil_file(&path)
                                                .unwrap_or_else(|_| {
                                                    veil_ir::LayerRegistry::builtin()
                                                });
                                        if let Ok(veil_ir::VeilFile::Package(pkg)) =
                                            veil_parser::parse_file_with_registry(
                                                &pkg_tokens,
                                                pkg_registry,
                                            )
                                        {
                                            resolved
                                                .push(veil_ir::resolve_package(&pkg, imp.alias));
                                        }
                                    }
                                    Err(e) => eprintln!("Warning: {}", e),
                                }
                            }
                            veil_ir::build_composition_ir(comp, &resolved)
                        }
                    };
                    (graph.nodes.len(), graph.edges.len())
                };

            let proj_name = veil_server::project_display_name(&project_root);
            println!(
                "✓ Serving project '{proj_name}' — {} file(s) ({} packages, {} layers; {} nodes, {} edges)",
                file_count, package_count, layer_count, node_count, edge_count
            );
            println!("  Root: {}", project_root.display());
            if file_count > 1 {
                for (i, entry) in file_entries.iter().enumerate() {
                    println!("  [{}] {}", i, entry.0.display());
                }
            }
            println!("  Layers (use): {}", registry.layers.join(", "));
            println!(
                "  Projects hub: {}  (veil projects list)",
                veil_server::default_projects_dir().display()
            );
            println!("  API: http://localhost:{}/api/ir", port);
            println!("  Files: http://localhost:{}/api/files", port);

            // AGT-010: proxy to remote serve when VEIL_REMOTE_URL is set.
            if let Ok(remote) = std::env::var("VEIL_REMOTE_URL") {
                println!("  Remote SourceStore: {remote}");
                println!("  (local paths ignored; IDE talks to remote package)");
                let provider = match veil_server::RemoteHttpProvider::from_env(registry.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Remote provider error: {e}");
                        std::process::exit(1);
                    }
                };
                let app = veil_server::build_router(provider);
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async move {
                    let listener = bind_serve_port(port).await;
                    println!("  Listening on port {port} (remote mode)");
                    if let Err(e) = axum::serve(listener, app).await {
                        eprintln!("Server error: {e}");
                        std::process::exit(1);
                    }
                });
                return;
            }

            let provider = veil_server::FilesystemProvider::with_files_in_project(
                file_entries,
                registry.clone(),
                Some(project_root),
            );
            let app = veil_server::build_router(provider);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = bind_serve_port(port).await;
                println!("  Listening on port {}", port);
                if let Err(e) = axum::serve(listener, app).await {
                    eprintln!("Server error: {e}");
                    std::process::exit(1);
                }
            });
        }
    }
}

/// Bind `0.0.0.0:port` with a clear error on AddrInUse (no panic).
async fn bind_serve_port(port: u16) -> tokio::net::TcpListener {
    match tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!(
                "error: port {port} is already in use.\n\
                 \n\
                 Another process (often a previous `veil serve`) is listening there.\n\
                 \n\
                   # free the port (if it's an old veil):\n\
                   fuser -k {port}/tcp\n\
                   # or pick another port:\n\
                   veil serve <path> -p 3002\n\
                   make serve PORT=3002\n"
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: failed to bind 0.0.0.0:{port}: {e}");
            std::process::exit(1);
        }
    }
}
