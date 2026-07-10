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
        /// Suppress multi-target debt warnings (features not honest on other targets).
        /// Debt warnings are emitted by default when `-t rust`.
        #[arg(long)]
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
    /// Start the visualization server
    Serve {
        /// Path to a .veil file or directory containing .veil files
        file: PathBuf,
        /// Port to serve on
        #[arg(short, long, default_value = "3001")]
        port: u16,
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

fn parse_solution_or_exit(source: &str, file: &std::path::Path) -> (veil_ir::Solution, LayerRegistry) {
    let tokens = veil_parser::lex(source);
    let registry = registry_for(file);
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
    for (id, item) in index {
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
            no_target_debt,
            dump_ir,
            emit_templates,
            list_capabilities,
            deny_escape_hatches,
            escape_summary,
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
            let (sol, registry) = parse_solution_or_exit(&source, &file);

            let mut result = veil_ir::check_solution(&sol, &registry);

            // Target capability matrix (CHK-005)
            result.diagnostics.extend(veil_codegen::check_target_capabilities(
                &sol,
                &registry,
                codegen_target,
            ));
            // Multi-target debt: warn about TS gaps when primary target is Rust
            if !no_target_debt && codegen_target == veil_codegen::CodegenTarget::Rust {
                result
                    .diagnostics
                    .extend(veil_codegen::check_multi_target_debt(&sol, &registry));
            }

            // Escape-hatch debt (CHK-006): already included in check_solution as warnings
            if deny_escape_hatches {
                veil_ir::promote_escape_hatches(&mut result.diagnostics);
            }

            veil_ir::sort_diagnostics(&mut result.diagnostics);

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

            let hatch_counts =
                veil_ir::EscapeHatchSummary::from_diagnostics(&result.diagnostics);

            if result.diagnostics.is_empty() {
                eprintln!(
                    "ok — {} node(s), {} edge(s), 0 diagnostics",
                    result.graph.nodes.len(),
                    result.graph.edges.len()
                );
            } else {
                for d in &result.diagnostics {
                    // stdout for machine-friendly piping; include file path
                    println!("{}: {}", file_display, veil_ir::format_diagnostic_line(d));
                }
                eprintln!(
                    "{} error(s), {} warning(s)",
                    result.error_count(),
                    result.warning_count()
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
                    eprintln!("Unknown target '{}'. Use: rust, typescript", target);
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
                    // Packages generate typed API clients (TS) or full Rust crates
                    match codegen_target {
                        veil_codegen::CodegenTarget::TypeScript => {
                            let project = veil_codegen::typescript::generate_api_client_from_package(pkg);
                            project.files.into_iter()
                                .map(|f| veil_codegen::GeneratedFile { path: f.path, content: f.content })
                                .collect()
                        }
                        veil_codegen::CodegenTarget::Rust => {
                            // Convert Package to Solution for Rust codegen
                            let sol = &veil_ir::ast::Solution {
                                name: pkg.name.clone(),
                                span: pkg.span,
                                uses: pkg.uses.clone(),
                                items: pkg.items.clone(),
                                expose: pkg.expose.clone(),
                            };
                            veil_codegen::generate_for_target(sol, &registry, codegen_target)
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

            // Run formatters on generated files
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
        Commands::Serve { file, port } => {
            // Collect .veil files: single file or directory scan
            let veil_files: Vec<PathBuf> = if file.is_dir() {
                let mut found: Vec<PathBuf> = std::fs::read_dir(&file)
                    .expect("Failed to read directory")
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| {
                        entry.path().extension()
                            .map(|ext| ext == "veil")
                            .unwrap_or(false)
                    })
                    .map(|entry| entry.path())
                    .collect();
                found.sort();
                if found.is_empty() {
                    eprintln!("No .veil files found in {}", file.display());
                    std::process::exit(1);
                }
                found
            } else {
                vec![file.clone()]
            };

            // Load and parse the first file to set up the registry
            let first_file = &veil_files[0];
            let first_source = std::fs::read_to_string(first_file).expect("Failed to read file");
            let registry = registry_for(first_file);

            // Load all files
            let file_entries: Vec<(PathBuf, String, bool)> = veil_files.iter().map(|path| {
                let source = std::fs::read_to_string(path).expect("Failed to read file");
                let editable = !source.trim_start().starts_with("pkg ");
                (path.clone(), source, editable)
            }).collect();

            let file_count = file_entries.len();

            // Build IR from the first file for initial display
            let tokens = veil_parser::lex(&first_source);
            let veil_file = match veil_parser::parse_file_with_registry(&tokens, registry.clone()) {
                Ok(f) => f,
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors { eprintln!("  {}", err); }
                    std::process::exit(1);
                }
            };

            let graph = match &veil_file {
                veil_ir::VeilFile::Solution(sol) => veil_ir::build_ir(sol),
                veil_ir::VeilFile::Package(pkg) => {
                    let sol = veil_ir::Solution {
                        name: pkg.name.clone(),
                        span: pkg.span,
                        uses: Vec::new(),
                        items: pkg.items.clone(),
                        expose: pkg.expose.clone(),
                    };
                    veil_ir::build_ir(&sol)
                }
                veil_ir::VeilFile::Composition(comp) => {
                    let search_dir = first_file.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
                    let search_paths = vec![search_dir];
                    let mut resolved = Vec::new();
                    let found = veil_ir::find_package_files(&comp.imports, &search_paths);
                    for result in found {
                        match result {
                            Ok((imp, path)) => {
                                let pkg_source = std::fs::read_to_string(&path)
                                    .expect("Failed to read package");
                                let pkg_tokens = veil_parser::lex(&pkg_source);
                                let pkg_registry = veil_ir::LayerRegistry::for_veil_file(&path)
                                    .unwrap_or_else(|_| veil_ir::LayerRegistry::builtin());
                                if let Ok(veil_ir::VeilFile::Package(pkg)) =
                                    veil_parser::parse_file_with_registry(&pkg_tokens, pkg_registry)
                                {
                                    resolved.push(veil_ir::resolve_package(&pkg, imp.alias));
                                }
                            }
                            Err(e) => eprintln!("Warning: {}", e),
                        }
                    }
                    veil_ir::build_composition_ir(comp, &resolved)
                }
            };

            let node_count = graph.nodes.len();
            let edge_count = graph.edges.len();

            println!("✓ Serving {} file(s) ({} nodes, {} edges)", file_count, node_count, edge_count);
            if file_count > 1 {
                for (i, entry) in file_entries.iter().enumerate() {
                    println!("  [{}] {}", i, entry.0.display());
                }
            }
            println!("  Layers: {}", registry.layers.join(", "));
            println!("  API: http://localhost:{}/api/ir", port);
            println!("  Files: http://localhost:{}/api/files", port);

            let provider = veil_server::FilesystemProvider::with_files(file_entries, registry.clone());
            let app = veil_server::build_router(provider);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
                    .await
                    .unwrap();
                println!("  Listening on port {}", port);
                axum::serve(listener, app).await.unwrap();
            });
        }
    }
}
