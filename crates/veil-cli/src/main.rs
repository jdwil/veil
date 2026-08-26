//! VEIL CLI — parse, validate, generate, and serve VEIL files.
//!
//! All vocabulary comes from `.layer` files referenced by the input's `use`
//! lines. The CLI contains zero domain knowledge: it loads the layer registry,
//! hands it to the parser, and serves palette metadata straight from it.

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::{Path, PathBuf};

use veil_ir::LayerRegistry;

/// Write `content` via temp file + rename so concurrent readers (e.g. Vite
/// watching `src/app.html`) never observe a truncated intermediate.
/// Skips writing if the file already has identical content (prevents
/// unnecessary Vite HMR triggers and dev server crashes).
/// Returns true if the file was actually written (content changed).
fn write_file_atomic(path: &Path, content: &[u8]) -> std::io::Result<bool> {
    // Skip if file already has identical content
    if path.exists() {
        if let Ok(existing) = std::fs::read(path) {
            if existing == content {
                return Ok(false);
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("veilgen")
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(true)
}


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
        /// Print layer pass execution trace (which rules matched which nodes)
        #[arg(long)]
        trace_passes: bool,
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
        /// Keep existing `crates/*` that this gen did not emit.
        /// Used by multi-package devloop so intermediate package gens do not
        /// delete sibling context crates before gen-harness runs.
        #[arg(long, default_value_t = false)]
        no_prune: bool,
        /// Print layer pass execution trace (which rules matched which nodes)
        #[arg(long)]
        trace_passes: bool,
    },
    /// Generate a combined harness binary for multiple VEIL packages (local dev).
    /// Used by the devloop for multi-package workspaces.
    GenHarness {
        /// Paths to .veil package files
        files: Vec<PathBuf>,
        /// Output directory (the workspace root, e.g. generated/backend)
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
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
    /// Create a new VEIL component from a template (extension, layer, etc.)
    New {
        #[command(subcommand)]
        what: NewCmd,
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
    /// Connect a local ACP agent to the runtime for LLM reasoning.
    ///
    /// Spawns a local agent process and bridges its stdin/stdout to the runtime's
    /// `/api/agent/acp` WebSocket endpoint. The agent provides LLM reasoning;
    /// tool execution still happens on the runtime.
    Agent {
        #[command(subcommand)]
        action: AgentCmd,
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
        /// Cargo features to enable when creating a temp project (`cargo add --features`)
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
    },
    /// Run tests defined in VEIL test blocks (parse → codegen → run)
    Test {
        /// Path to the .veil file(s)
        file: Option<PathBuf>,
        /// Filter test cases by name
        #[arg(long)]
        filter: Option<String>,
        /// Only unit tests (tests blocks without `mount`)
        #[arg(long)]
        unit: bool,
        /// Only component tests (tests blocks with `mount`)
        #[arg(long)]
        component: bool,
        /// Run all *.test.veil scenario files
        #[arg(long)]
        scenarios: bool,
        /// Run all integration test blocks
        #[arg(long)]
        integration: bool,
        /// Target language (rust, typescript)
        #[arg(short = 't', long)]
        target: Option<String>,
        /// Output results as JSON
        #[arg(long)]
        json: bool,
        /// Watch mode: re-run tests on .veil file changes
        #[arg(long)]
        watch: bool,
        /// Report test coverage at the VEIL source level
        #[arg(long)]
        coverage: bool,
        /// Update snapshot files (passed through to target runner)
        #[arg(long)]
        update_snapshots: bool,
    },
}



#[derive(Subcommand)]
enum NewCmd {
    /// Scaffold a new VEIL extension package with templates
    Extension {
        /// Extension name (kebab-case, e.g. "my-reaction")
        name: String,
        /// Extension kind (reaction, signal, activation, ui-panel)
        #[arg(short, long, default_value = "reaction")]
        kind: String,
        /// Directory to create in (default: current directory)
        #[arg(short, long)]
        dir: Option<PathBuf>,
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

#[derive(Subcommand)]
enum AgentCmd {
    /// Connect a local ACP agent to the runtime.
    ///
    /// Default command: `kiro-cli acp --agent hive --trust-all-tools`
    /// Override with --command.
    Connect {
        /// Command to spawn as the ACP agent process.
        /// The process must read ACP turn_request frames from stdin (JSON lines)
        /// and write content_delta / tool_use / turn_complete frames to stdout.
        #[arg(long, default_value = "kiro-cli acp --agent hive --trust-all-tools")]
        command: String,
        /// Runtime URL (default: read from ~/.veil/config.json or http://localhost:8080)
        #[arg(long)]
        runtime_url: Option<String>,
        /// Auth token (default: read from ~/.veil/config.json)
        #[arg(long)]
        token: Option<String>,
        /// Display name for this agent shown in the dashboard
        #[arg(long, default_value = "Kiro")]
        agent_name: String,
    },
    /// Show the current agent connection status.
    Status {
        /// Runtime URL (default: read from ~/.veil/config.json or http://localhost:8080)
        #[arg(long)]
        runtime_url: Option<String>,
    },
}

/// Load the layer registry for a .veil file, exiting on layer errors.
fn registry_for(file: &std::path::Path) -> LayerRegistry {
    let (layer_resolver, source_resolver, stub_resolver) = build_external_resolvers();
    match LayerRegistry::for_veil_file_with_resolvers(file, layer_resolver, source_resolver, stub_resolver) {
        Ok(reg) => reg,
        Err(e) => {
            eprintln!("Layer error: {}", e);
            std::process::exit(1);
        }
    }
}

/// When VEIL_DDB_TABLE is set, build DDB/S3-backed resolvers for layers and package source.
/// This enables `veil gen` to work in deployed environments without filesystem access to layers.
fn build_external_resolvers() -> (
    Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
) {
    let Some(resolver) = veil_local::SourceResolver::from_env() else {
        return (None, None, None);
    };
    if std::env::var("VEIL_DEV").is_ok() {
        eprintln!("  [resolver] DDB-backed source resolver active (table: {})",
            std::env::var("VEIL_DDB_TABLE").unwrap_or_default());
    }
    let resolver = std::sync::Arc::new(resolver);
    let r1 = resolver.clone();
    let r2 = resolver.clone();
    let r3 = resolver;
    let layer_resolver: Box<dyn Fn(&str) -> Option<String> + Send + Sync> =
        Box::new(move |name| {
            let result = r1.resolve_layer(name);
            if std::env::var("VEIL_DEV").is_ok() {
                eprintln!("  [resolver] layer '{}' → {}", name,
                    if result.is_some() { "found" } else { "not found" });
            }
            result
        });
    let source_resolver: Box<dyn Fn(&str) -> Option<String> + Send + Sync> =
        Box::new(move |name| {
            let result = r2.resolve_package_source(name);
            if std::env::var("VEIL_DEV").is_ok() {
                eprintln!("  [resolver] source '{}' → {}", name,
                    if result.is_some() { "found" } else { "not found" });
            }
            result
        });
    let stub_resolver: Box<dyn Fn(&str) -> Option<String> + Send + Sync> =
        Box::new(move |name| {
            let result = r3.resolve_stub(name);
            if std::env::var("VEIL_DEV").is_ok() {
                eprintln!("  [resolver] stub '{}' → {}", name,
                    if result.is_some() { "found" } else { "not found" });
            }
            result
        });
    (Some(layer_resolver), Some(source_resolver), Some(stub_resolver))
}

/// After single-file `veil gen`, remove `crates/<name>/` directories that this
/// generation did not emit. Prevents leftover crates (e.g. `iaaa` from an old
/// multi-package harness) from cluttering the output and confusing builds.
fn prune_stale_gen_crates(output: &std::path::Path, files: &[veil_codegen::GeneratedFile]) {
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in files {
        let path = f.path.replace('\\', "/");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 && parts[0] == "crates" && !parts[1].is_empty() {
            keep.insert(parts[1].to_string());
        }
    }
    prune_crates_except(output, &keep);
}

/// Remove `crates/<name>/` entries not listed in `keep`.
fn prune_crates_except(output: &std::path::Path, keep: &std::collections::HashSet<String>) {
    let crates_dir = output.join("crates");
    if !crates_dir.is_dir() || keep.is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&crates_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if keep.contains(name) {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => eprintln!(
                "  pruned stale crate not in this gen: {}",
                path.strip_prefix(output).unwrap_or(&path).display()
            ),
            Err(e) => eprintln!("  warning: could not prune {}: {e}", path.display()),
        }
    }
}

/// After multi-package gen-harness: set workspace `members` to every crate under
/// `crates/`, and ensure veil_bin path-depends on each context crate.
fn finalize_multi_package_workspace(output: &std::path::Path) {
    let crates_dir = output.join("crates");
    if !crates_dir.is_dir() {
        return;
    }
    let mut members: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("Cargo.toml").is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    members.push(name.to_string());
                }
            }
        }
    }
    if members.is_empty() {
        return;
    }
    members.sort_by(|a, b| {
        let order = |s: &str| -> u8 {
            match s {
                "veil_shared" => 0,
                "veil_bin" => 1,
                _ => 2,
            }
        };
        order(a).cmp(&order(b)).then(a.cmp(b))
    });

    let cargo_path = output.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_path) {
        let members_str = members
            .iter()
            .map(|m| format!("    \"crates/{m}\""))
            .collect::<Vec<_>>()
            .join(",\n");
        if let Some(start) = content.find("members = [") {
            if let Some(end) = content[start..].find(']') {
                let before = &content[..start];
                let after = &content[start + end + 1..];
                let new_content = format!("{before}members = [\n{members_str}\n]{after}");
                let _ = std::fs::write(&cargo_path, new_content);
            }
        }
    }

    // veil_bin should path-depend on every context crate (gen-harness usually
    // already wrote these; fill gaps if a crate exists without a dep line).
    let bin_cargo = crates_dir.join("veil_bin").join("Cargo.toml");
    if let Ok(bin_content) = std::fs::read_to_string(&bin_cargo) {
        let mut new_bin = bin_content.clone();
        for m in &members {
            if m == "veil_shared" || m == "veil_bin" {
                continue;
            }
            let key_start = format!("{m} = ");
            if new_bin.lines().any(|l| l.trim_start().starts_with(&key_start)) {
                continue;
            }
            let dep_line = format!("{m} = {{ path = \"../{m}\" }}\n");
            if let Some(deps_pos) = new_bin.find("[dependencies]") {
                let insert_pos = new_bin[deps_pos..]
                    .find('\n')
                    .map(|p| deps_pos + p + 1)
                    .unwrap_or(new_bin.len());
                let after_deps = &new_bin[insert_pos..];
                let section_end = after_deps.find("\n[").unwrap_or(after_deps.len());
                new_bin.insert_str(insert_pos + section_end, &dep_line);
            }
        }
        if new_bin != bin_content {
            let _ = std::fs::write(&bin_cargo, new_bin);
        }
    }
}

/// Merge stub crate declarations into `[workspace.dependencies]` of an existing
/// workspace Cargo.toml (used by gen-harness so veil_bin workspace=true deps resolve).
fn merge_stub_workspace_deps(workspace_toml: &str, stubs: &[(String, String, Vec<String>, Vec<(String, String)>)]) -> String {
    // stubs: (name, version, features, cargo_deps)
    if stubs.is_empty() {
        return workspace_toml.to_string();
    }
    let Some(pos) = workspace_toml.find("[workspace.dependencies]") else {
        return workspace_toml.to_string();
    };
    let after_header = pos + "[workspace.dependencies]".len();
    let rest = &workspace_toml[after_header..];
    let section_end_rel = rest.find("\n[").unwrap_or(rest.len());
    let section = &rest[..section_end_rel];
    let after_section = &rest[section_end_rel..];

    let mut extra = String::new();
    for (name, version, features, cargo_deps) in stubs {
        if !section.contains(name) && !extra.contains(name) {
            if features.is_empty() {
                extra.push_str(&format!("\n{name} = \"{version}\""));
            } else {
                let feats: Vec<String> = features.iter().map(|f| format!("\"{f}\"")).collect();
                extra.push_str(&format!(
                    "\n{name} = {{ version = \"{version}\", features = [{}] }}",
                    feats.join(", ")
                ));
            }
        }
        for (dep_name, dep_ver) in cargo_deps {
            if !section.contains(dep_name) && !extra.contains(dep_name) {
                extra.push_str(&format!("\n{dep_name} = \"{dep_ver}\""));
            }
        }
    }
    if extra.is_empty() {
        return workspace_toml.to_string();
    }
    // Insert before end of workspace.dependencies section
    let mut out = String::with_capacity(workspace_toml.len() + extra.len());
    out.push_str(&workspace_toml[..after_header]);
    out.push_str(section);
    if !section.ends_with('\n') && !section.is_empty() {
        out.push('\n');
    }
    // extra starts with \n
    out.push_str(extra.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(after_section);
    out
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

#[cfg(test)]
mod rustdoc_type_tests {
    use super::{rustdoc_module_under_crate, rustdoc_type_to_veil};
    use serde_json::json;

    #[test]
    fn borrowed_slice_of_hashmap_is_list_map_externally_tagged() {
        let ty = json!({
            "borrowed_ref": {
                "lifetime": null,
                "is_mutable": false,
                "type": {
                    "slice": {
                        "resolved_path": {
                            "name": "HashMap",
                            "path": "std::collections::HashMap",
                            "args": {
                                "angle_bracketed": {
                                    "args": [
                                        { "type": { "resolved_path": { "path": "alloc::string::String" } } },
                                        { "type": { "resolved_path": { "path": "aws_sdk_dynamodb::types::AttributeValue" } } }
                                    ]
                                }
                            }
                        }
                    }
                }
            }
        });
        assert_eq!(
            rustdoc_type_to_veil(&ty),
            "List<Map<Str, AttributeValue>>"
        );
    }

    #[test]
    fn borrowed_slice_of_hashmap_is_list_map_internally_tagged() {
        let ty = json!({
            "kind": "borrowed_ref",
            "inner": {
                "lifetime": null,
                "is_mutable": false,
                "type": {
                    "kind": "slice",
                    "inner": {
                        "kind": "resolved_path",
                        "inner": {
                            "name": "HashMap",
                            "path": "std::collections::HashMap",
                            "args": {
                                "kind": "angle_bracketed",
                                "inner": {
                                    "args": [
                                        {
                                            "kind": "type",
                                            "inner": { "kind": "resolved_path", "inner": { "path": "alloc::string::String" } }
                                        },
                                        {
                                            "kind": "type",
                                            "inner": { "kind": "resolved_path", "inner": { "path": "aws_sdk_dynamodb::types::AttributeValue" } }
                                        }
                                    ]
                                }
                            }
                        }
                    }
                }
            }
        });
        assert_eq!(
            rustdoc_type_to_veil(&ty),
            "List<Map<Str, AttributeValue>>"
        );
    }

    #[test]
    fn unknown_shape_is_not_silently_str() {
        let ty = json!({ "not_a_rustdoc_type": true });
        assert_eq!(rustdoc_type_to_veil(&ty), "Unknown");
    }

    #[test]
    fn rustdoc_path_primitives_is_module() {
        let paths = json!({
            "0:1": { "path": ["aws_sdk_lambda", "primitives", "Blob"] },
            "0:2": { "path": ["aws_sdk_lambda", "Client"] },
            "0:3": { "path": ["aws_sdk_sns", "types", "MessageAttributeValue"] }
        });
        let map = paths.as_object().unwrap();
        assert_eq!(
            rustdoc_module_under_crate(Some(map), "0:1", "aws_sdk_lambda", "Blob")
                .as_deref(),
            Some("primitives")
        );
        assert_eq!(
            rustdoc_module_under_crate(Some(map), "0:2", "aws_sdk_lambda", "Client"),
            None
        );
        assert_eq!(
            rustdoc_module_under_crate(Some(map), "0:3", "aws_sdk_sns", "MessageAttributeValue")
                .as_deref(),
            Some("types")
        );
    }

    #[test]
    fn impl_into_string_is_str() {
        let ty = json!({
            "impl_trait": [
                {
                    "trait_bound": {
                        "trait": {
                            "path": "core::convert::Into",
                            "args": {
                                "angle_bracketed": {
                                    "args": [
                                        { "type": { "resolved_path": { "path": "alloc::string::String" } } }
                                    ]
                                }
                            }
                        }
                    }
                }
            ]
        });
        assert_eq!(rustdoc_type_to_veil(&ty), "Str");
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
        veil_ir::VeilFile::Package(mut pkg) => {
            // Implicit adapt: `use X` + companion X.veil (hub siblings + [dependencies]).
            let search = veil_ir::adapt_search_paths_for_file(file);
            veil_ir::inject_implicit_adapts(&mut pkg, &search);
            if !pkg.adapts.is_empty() || !pkg.patches.is_empty() {
                match merge_package_or_exit(&pkg, file) {
                    Ok(sol) => (sol, registry),
                    Err(()) => std::process::exit(1),
                }
            } else {
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

/// Load macros from use'd packages for cross-package macro expansion.
///
/// Resolution order for each `use X`:
/// 1. Filesystem: find X/main.veil or X.veil via adapt search paths
/// 2. Source resolver (DDB/S3): registry.source_resolver("X")
///
/// Returns a HashMap of macro_name → MacroDef for all external macros found.
fn load_external_macros(
    pkg: &veil_ir::Package,
    leaf_path: &std::path::Path,
    registry: &veil_ir::LayerRegistry,
) -> std::collections::HashMap<String, veil_ir::ast::MacroDef> {
    use std::collections::HashMap;
    let mut macros: HashMap<String, veil_ir::ast::MacroDef> = HashMap::new();
    let search = veil_ir::adapt_search_paths_for_file(leaf_path);

    for use_ref in &pkg.uses {
        let name = &use_ref.package_name;
        // Skip layer/stub-only uses (they won't have macros)
        if veil_ir::is_adapt_denied(name) {
            continue;
        }

        // Try filesystem first
        let source = if let Some(path) = veil_ir::find_package_source(name, &search) {
            std::fs::read_to_string(&path).ok()
        } else {
            None
        };

        // Fall back to source_resolver (DDB/S3)
        let source = source.or_else(|| {
            registry.source_resolver.as_ref().and_then(|resolver| resolver(name))
        });

        let Some(src) = source else { continue };

        // Parse the source to extract macros. Use the same registry as the parent
        // package so layer-specific keywords are recognized.
        let tokens = veil_parser::lex(&src);
        let found_macros = match veil_parser::parse_file_with_registry(&tokens, registry.clone()) {
            Ok(veil_ir::VeilFile::Package(dep_pkg)) => {
                dep_pkg.macros
            }
            Ok(_) => Vec::new(),
            Err(_) => {
                // Full parse failed (e.g. rich enum variants not supported).
                // Fall back: re-lex and extract just macro definitions by parsing
                // only the macro blocks at the package top level.
                extract_macros_from_source(&src)
            }
        };
        for m in &found_macros {
            macros.insert(m.name.clone(), m.clone());
        }
    }

    macros
}

/// Extract macro definitions from source when full parsing fails.
/// Uses a simplified approach: wraps the macro definitions in a minimal pkg
/// that the parser can handle, skipping everything else.
fn extract_macros_from_source(source: &str) -> Vec<veil_ir::ast::MacroDef> {
    // Find macro blocks by scanning indentation: `  macro name(...)` at pkg body level.
    // Extract just those blocks and wrap in a minimal pkg for parsing.
    let mut macro_blocks = String::new();
    let mut in_macro = false;
    let mut macro_indent = 0;

    for line in source.lines() {
        if !in_macro {
            let trimmed = line.trim();
            if trimmed.starts_with("macro ") {
                in_macro = true;
                macro_indent = line.len() - line.trim_start().len();
                macro_blocks.push_str(line);
                macro_blocks.push('\n');
            }
        } else {
            // Continue collecting macro body: lines indented deeper than the macro keyword
            let line_indent = line.len() - line.trim_start().len();
            if line.trim().is_empty() {
                // Empty line inside macro body
                macro_blocks.push('\n');
            } else if line_indent > macro_indent {
                macro_blocks.push_str(line);
                macro_blocks.push('\n');
            } else {
                // End of macro block
                in_macro = false;
                // Check if this line starts a new macro
                if line.trim().starts_with("macro ") {
                    in_macro = true;
                    macro_indent = line_indent;
                    macro_blocks.push_str(line);
                    macro_blocks.push('\n');
                }
            }
        }
    }

    if macro_blocks.is_empty() {
        return Vec::new();
    }

    // Wrap in a minimal pkg and parse
    let wrapper = format!("pkg __macros__\n{}", macro_blocks);
    let tokens = veil_parser::lex(&wrapper);
    let reg = veil_ir::LayerRegistry::builtin();
    match veil_parser::parse_file_with_registry(&tokens, reg) {
        Ok(veil_ir::VeilFile::Package(pkg)) => pkg.macros,
        _ => Vec::new(),
    }
}

/// Load adapt bases and flatten to a Solution for check/codegen.
fn merge_package_or_exit(
    leaf: &veil_ir::Package,
    leaf_path: &std::path::Path,
) -> Result<veil_ir::Solution, ()> {
    // R20: hub siblings + veil.toml [dependencies] roots
    let search = veil_ir::adapt_search_paths_for_file(leaf_path);
    let project_root = veil_ir::find_project_root(leaf_path);
    // Unified use resolution: inject implicit adapts for uses with companion .veil.
    let mut leaf = leaf.clone();
    veil_ir::inject_implicit_adapts(&mut leaf, &search);
    let load = |name: &str| -> Result<veil_ir::Package, String> {
        let path = veil_ir::find_package_source(name, &search).ok_or_else(|| {
            veil_ir::missing_package_hint(name, project_root.as_deref())
        })?;
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
    match veil_ir::merge_adapted_package(&leaf, load) {
        Ok(merged) => {
            if !merged.chain.is_empty() && merged.chain.len() > 1 {
                eprintln!(
                    "adapt chain: {}",
                    merged.chain.join(" → ")
                );
            }
            // Prefer package_as_solution so raw blocks (template/script/style)
            // survive adapt flatten. serialize→parse drops raw surfaces and was
            // emptying designkit components after `use designkit` merge.
            Ok(veil_ir::package_as_solution(&merged.package))
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
fn generate_stub(
    crate_name: &str,
    project_dir: &std::path::Path,
    features: &[String],
) -> Result<String, String> {
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
        let mut add_args = vec!["add".to_string(), crate_name.to_string()];
        if !features.is_empty() {
            add_args.push("--features".into());
            add_args.push(features.join(","));
        }
        let add = Command::new("cargo")
            .args(&add_args)
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
    let features = read_dep_features(&work_dir, crate_name);

    // Primary package (may be a thin re-export facade, e.g. sqlx → sqlx-core).
    let primary = rustdoc_json_for_package(&work_dir, crate_name)?;
    let mut stub = convert_rustdoc_json_to_stub(&primary, crate_name, &features)?;

    // Facade crates (sqlx, etc.) re-export from *-core; rustdoc index is almost empty.
    // When sparse, also document the -core package and merge API shapes under the
    // original stub name so `use sqlx` still works.
    if stub_is_sparse(&stub) {
        let core_pkg = format!("{crate_name}-core");
        eprintln!("  Primary stub sparse — trying dependency {core_pkg}…");
        match rustdoc_json_for_package(&work_dir, &core_pkg) {
            Ok(core_json) => {
                // Keep user-facing crate name / features; take API from core.
                let core_stub =
                    convert_rustdoc_json_to_stub(&core_json, crate_name, &features)?;
                stub = merge_stub_prefer_richer(stub, core_stub);
            }
            Err(e) => {
                eprintln!("  warning: could not document {core_pkg}: {e}");
            }
        }
    }

    Ok(stub)
}

fn rustdoc_json_for_package(work_dir: &std::path::Path, package: &str) -> Result<String, String> {
    use std::process::Command;
    let output = Command::new("cargo")
        .args([
            "+nightly",
            "rustdoc",
            "-p",
            package,
            "--",
            "--output-format",
            "json",
            "-Z",
            "unstable-options",
        ])
        .current_dir(work_dir)
        .output()
        .map_err(|e| format!("Failed to run cargo rustdoc -p {package}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo rustdoc -p {package} failed: {stderr}"));
    }

    let file_name = package.replace('-', "_");
    let json_path = work_dir
        .join("target")
        .join("doc")
        .join(format!("{file_name}.json"));
    if json_path.exists() {
        return std::fs::read_to_string(&json_path)
            .map_err(|e| format!("Cannot read {}: {e}", json_path.display()));
    }
    let alt = work_dir
        .join("target")
        .join("doc")
        .join(format!("{package}.json"));
    std::fs::read_to_string(&alt).map_err(|e| {
        format!(
            "JSON not found for {package} at {} or {}: {e}",
            json_path.display(),
            alt.display()
        )
    })
}

/// True when rustdoc produced almost no API surface (typical re-export facade).
fn stub_is_sparse(stub: &str) -> bool {
    let struct_count = stub.lines().filter(|l| l.trim_start().starts_with("struct ")).count();
    let trait_count = stub.lines().filter(|l| l.trim_start().starts_with("trait ")).count();
    struct_count + trait_count < 5
}

/// Prefer the richer stub for structs/traits; keep header/policy from either.
fn merge_stub_prefer_richer(a: String, b: String) -> String {
    if stub_is_sparse(&a) && !stub_is_sparse(&b) {
        b
    } else if stub_is_sparse(&b) {
        a
    } else {
        // Both rich — prefer b (core) but keep a's header if versions differ
        b
    }
}

/// Read enabled features for `crate_name` from a workspace Cargo.toml (post `cargo add`).
fn read_dep_features(work_dir: &std::path::Path, crate_name: &str) -> Vec<String> {
    let cargo_toml = work_dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(cargo_toml) else {
        return Vec::new();
    };
    // Match [dependencies.crate] features = [...]  or crate = { features = [...] }
    let mut features = Vec::new();
    let mut in_dep_table = false;
    let table_hdr = format!("[dependencies.{}]", crate_name);
    for line in text.lines() {
        let t = line.trim();
        if t == table_hdr {
            in_dep_table = true;
            continue;
        }
        if t.starts_with('[') {
            in_dep_table = false;
        }
        if in_dep_table {
            if let Some(rest) = t.strip_prefix("features") {
                let rest = rest.trim().trim_start_matches('=').trim();
                if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    for f in inner.split(',') {
                        let f = f.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !f.is_empty() {
                            features.push(f);
                        }
                    }
                }
            }
        }
        // Inline: sqlx = { version = "…", features = ["a", "b"] }
        if t.starts_with(crate_name) && t.contains("features") {
            if let Some(idx) = t.find("features") {
                let rest = &t[idx..];
                if let Some(lb) = rest.find('[') {
                    if let Some(rb) = rest.find(']') {
                        for f in rest[lb + 1..rb].split(',') {
                            let f = f.trim().trim_matches('"').trim_matches('\'').to_string();
                            if !f.is_empty() && !features.contains(&f) {
                                features.push(f);
                            }
                        }
                    }
                }
            }
        }
    }
    features
}

/// Convert rustdoc JSON to .stub file format.
///
/// Emits API shapes **plus** generic codegen policy inferred from the crate
/// (traits like `FromRow`/`Type`, free fns `query`/`query_as`, type aliases).
/// The engine consumes that policy without naming any particular crate.
fn convert_rustdoc_json_to_stub(
    json_str: &str,
    crate_name: &str,
    cargo_features: &[String],
) -> Result<String, String> {
    let data: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("Invalid JSON: {}", e))?;

    let version = data.get("crate_version")
        .and_then(|v| v.as_str())
        .unwrap_or("*");

    let index = data.get("index")
        .and_then(|v| v.as_object())
        .ok_or("No index in JSON")?;

    let rust_crate = crate_name.replace('-', "_");

    // Collect structs and their impl items
    let mut struct_ids: Vec<(String, String)> = Vec::new(); // (id, name)
    let mut struct_impls: std::collections::HashMap<String, Vec<MethodSigInfo>> = std::collections::HashMap::new(); // name → method infos
    // rustdoc `paths[id].path` → module under the crate (`primitives` for Blob).
    let mut struct_module_paths: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let rustdoc_paths = data.get("paths").and_then(|v| v.as_object());
    let mut enum_defs: Vec<(String, Vec<String>)> = Vec::new(); // (name, variants)
    let mut trait_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut free_fns: std::collections::BTreeMap<String, MethodSigInfo> = std::collections::BTreeMap::new(); // name → method info
    // Type alias: alias_name → last segment of target (e.g. PgPool → Pool)
    let mut type_aliases: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for (id, item) in index {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let vis = item.get("visibility").and_then(|v| v.as_str()).unwrap_or("");
        if vis != "public" { continue; }

        let inner = match item.get("inner").and_then(|v| v.as_object()) {
            Some(i) => i,
            None => continue,
        };

        // Public free functions (crate-level only). Methods also appear as
        // `function` items in rustdoc; skip any signature that takes `self`.
        if inner.contains_key("function") {
            if name.chars().next().map(|c| c.is_lowercase()).unwrap_or(false)
                && !function_item_has_self(item)
            {
                if let Some(info) = extract_method_sig_with_info(item) {
                    free_fns.insert(name.to_string(), info);
                }
            }
        }

        // Type aliases (e.g. PgPool = Pool<Postgres>)
        if let Some(alias_data) = inner.get("type_alias").or_else(|| inner.get("typedef")) {
            if let Some(target) = alias_data
                .as_object()
                .and_then(|o| o.get("type_"))
                .or_else(|| alias_data.as_object().and_then(|o| o.get("type")))
            {
                let target_veil = rustdoc_type_to_veil(target);
                // Base name without generics: Pool<…> → Pool
                let base = target_veil.split('<').next().unwrap_or(&target_veil).to_string();
                if !name.is_empty() && !base.is_empty() && name != base {
                    type_aliases.insert(name.to_string(), base);
                }
            }
        }

        // Collect public structs
        if inner.contains_key("struct") {
            struct_ids.push((id.clone(), name.to_string()));
            if let Some(module) = rustdoc_module_under_crate(rustdoc_paths, id, &rust_crate, name)
            {
                struct_module_paths.insert(name.to_string(), module);
            }
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
                                                    if let Some(info) = extract_method_sig_with_info(method) {
                                                        struct_impls.entry(name.to_string())
                                                            .or_default()
                                                            .push(info);
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

        // Collect public enums with their variants
        if inner.contains_key("enum") {
            if let Some(enum_data) = inner.get("enum").and_then(|v| v.as_object()) {
                let mut variants = Vec::new();
                if let Some(variant_ids) = enum_data.get("variants").and_then(|v| v.as_array()) {
                    for vid in variant_ids {
                        let vid_str = vid.as_u64().map(|n| n.to_string())
                            .or_else(|| vid.as_str().map(|s| s.to_string()));
                        if let Some(vid_str) = vid_str {
                            if let Some(variant_item) = index.get(&vid_str) {
                                let vname = variant_item.get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown");
                                // Check variant kind: unit, tuple, or struct
                                if let Some(v_inner) = variant_item.get("inner")
                                    .and_then(|v| v.as_object())
                                    .and_then(|o| o.get("variant"))
                                    .and_then(|v| v.as_object())
                                {
                                    if let Some(kind) = v_inner.get("kind") {
                                        if let Some(tuple_fields) = kind.get("tuple").and_then(|v| v.as_array()) {
                                            let types: Vec<String> = tuple_fields.iter().filter_map(|f| {
                                                let fid = f.as_u64().map(|n| n.to_string())
                                                    .or_else(|| f.as_str().map(|s| s.to_string()))?;
                                                let field_item = index.get(&fid)?;
                                                let field_inner = field_item.get("inner")?
                                                    .as_object()?
                                                    .get("struct_field")?;
                                                Some(rustdoc_type_to_veil(field_inner))
                                            }).collect();
                                            if types.is_empty() {
                                                variants.push(vname.to_string());
                                            } else {
                                                variants.push(format!("{}({})", vname, types.join(", ")));
                                            }
                                        } else {
                                            variants.push(vname.to_string());
                                        }
                                    } else {
                                        variants.push(vname.to_string());
                                    }
                                } else {
                                    variants.push(vname.to_string());
                                }
                            }
                        }
                    }
                }
                if !variants.is_empty() {
                    enum_defs.push((name.to_string(), variants));
                }
                // Also collect impls on this enum
                if let Some(impls) = enum_data.get("impls").and_then(|v| v.as_array()) {
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
                                    if impl_inner.get("trait_").is_some() { continue; }
                                    if let Some(items) = impl_inner.get("items").and_then(|v| v.as_array()) {
                                        for method_id in items {
                                            let method_id_str = method_id.as_u64().map(|n| n.to_string())
                                                .or_else(|| method_id.as_str().map(|s| s.to_string()));
                                            if let Some(mid) = method_id_str {
                                                if let Some(method) = index.get(&mid) {
                                                    let method_vis = method.get("visibility").and_then(|v| v.as_str()).unwrap_or("");
                                                    if method_vis != "public" { continue; }
                                                    if let Some(info) = extract_method_sig_with_info(method) {
                                                        struct_impls.entry(name.to_string())
                                                            .or_default()
                                                            .push(info);
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

        // Public traits (names for policy + method listing later)
        if inner.contains_key("trait") {
            if !name.is_empty() {
                trait_names.insert(name.to_string());
            }
        }
    }

    let fingerprint = veil_ir::content_fingerprint(json_str);
    let mut out = veil_ir::generated_stub_header(
        crate_name,
        version,
        cargo_features,
        Some(&fingerprint),
    );

    // ── Crate-level codegen policy (inferred; engine applies generically) ──
    if !cargo_features.is_empty() {
        out.push_str(&format!(
            "  cargo_features {}\n",
            cargo_features.join(", ")
        ));
    }
    if trait_names.contains("FromRow") {
        out.push_str(&format!("  row_type_derives {rust_crate}::FromRow\n"));
    }
    if trait_names.contains("Type") {
        out.push_str(&format!("  wrapper_type_derives {rust_crate}::Type\n"));
        // Transparent newtypes are the usual pairing for Type on single-field wrappers.
        out.push_str(&format!("  wrapper_type_attrs {rust_crate}(transparent)\n"));
    }
    // Type aliases: only meaningful newtype aliases to a real stub struct
    // (e.g. PgPool → Pool). Skip Box/Result/etc. noise from rustdoc.
    let struct_names: std::collections::HashSet<&str> =
        struct_ids.iter().map(|(_, n)| n.as_str()).collect();
    let skip_alias_bases = [
        "Box", "Result", "Option", "Vec", "Arc", "Rc", "Str", "Res!", "Unknown", "Error",
    ];
    // base → best alias (prefer non-Any* names: PgPool over AnyPool)
    let mut best_alias: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (alias, base) in &type_aliases {
        if skip_alias_bases.iter().any(|s| *s == base.as_str()) {
            continue;
        }
        if !struct_names.contains(base.as_str()) {
            continue;
        }
        // Alias should look like a refined name of the base (PgPool / MyPool).
        if !alias.ends_with(base.as_str()) {
            continue;
        }
        match best_alias.get(base) {
            None => {
                best_alias.insert(base.clone(), alias.clone());
            }
            Some(prev) if prev.starts_with("Any") && !alias.starts_with("Any") => {
                best_alias.insert(base.clone(), alias.clone());
            }
            Some(prev) if prev.starts_with("Any") == alias.starts_with("Any") && alias.len() < prev.len() => {
                best_alias.insert(base.clone(), alias.clone());
            }
            _ => {}
        }
    }
    for (base, alias) in &best_alias {
        // Prefer feature-specific pool aliases over generic Any*
        if base == "Pool"
            && alias.starts_with("Any")
            && cargo_features.iter().any(|f| f == "postgres")
        {
            continue;
        }
        out.push_str(&format!("  rust_name {base} {alias}\n"));
        out.push_str(&format!("  codegen_imports {rust_crate}::{alias}\n"));
    }
    // postgres feature ⇒ conventional PgPool alias (often only on facade, not in core rustdoc)
    if cargo_features.iter().any(|f| f == "postgres") && struct_names.contains("Pool") {
        if !best_alias
            .get("Pool")
            .map(|a| a == "PgPool")
            .unwrap_or(false)
        {
            out.push_str("  rust_name Pool PgPool\n");
            out.push_str(&format!("  codegen_imports {rust_crate}::PgPool\n"));
        }
    }
    // Connection pool harness when Pool exists
    let has_pool = struct_names.contains("Pool");
    let pool_rust = if cargo_features.iter().any(|f| f == "postgres") {
        "PgPool"
    } else {
        best_alias
            .get("Pool")
            .map(|s| s.as_str())
            .unwrap_or("Pool")
    };
    if has_pool {
        let has_connect_lazy = struct_impls.get("Pool")
            .map(|ms| ms.iter().any(|m| m.sig.contains("fn connect_lazy")))
            .unwrap_or(false);
        if has_connect_lazy || pool_rust != "Pool" {
            out.push_str(&format!(
                "  harness_field Pool \"\"\"\n{{\n    let url = std::env::var(\"DATABASE_URL\").unwrap_or_else(|_| \"postgres://localhost/test\".into());\n    {rust_crate}::{pool_rust}::connect_lazy(&url).expect(\"pool\")\n}}\n\"\"\"\n"
            ));
        }
    }
    // reqwest / HTTP clients: builder with connect + total timeouts and no
    // automatic redirects (hang/SSRF hygiene — do not follow off-origin).
    // Re-run `veil stub-gen reqwest` after changing this recipe; do not hand-edit
    // the .stub harness_field.
    if struct_names.contains("Client")
        && (crate_name == "reqwest"
            || struct_impls
                .get("Client")
                .map(|ms| ms.iter().any(|m| m.name == "new"))
                .unwrap_or(false))
        && !crate_name.starts_with("aws_sdk_")
        && !crate_name.starts_with("aws-sdk-")
    {
        if crate_name == "reqwest" {
            // Single-line format! body: `\` line-continuation in Rust strips
            // leading whitespace on the next source line (would lose indent).
            out.push_str(&format!(
                "  harness_field Client \"\"\"\n{{\n    {rust_crate}::Client::builder()\n        .connect_timeout(std::time::Duration::from_secs(5))\n        .timeout(std::time::Duration::from_secs(30))\n        .redirect({rust_crate}::redirect::Policy::none())\n        .build()\n        .expect(\"reqwest client\")\n}}\n\"\"\"\n"
            ));
        } else {
            out.push_str(&format!(
                "  harness_field Client \"\"\"\n{{\n    {rust_crate}::Client::new()\n}}\n\"\"\"\n"
            ));
        }
    }
    // AWS SDK clients (aws-sdk-*): load_defaults + optional AWS_ENDPOINT_URL
    // (LocalStack / custom endpoints). Do not hand-edit .stub for this — re-run
    // veil stub-gen so the recipe stays in sync with the crate surface.
    if (crate_name.starts_with("aws_sdk_") || crate_name.starts_with("aws-sdk-"))
        && struct_names.contains("Client")
    {
        out.push_str("  types_module types\n");
        out.push_str("  root_types Client\n");
        out.push_str("  cargo_deps aws-config=1.8\n");
        out.push_str(&format!(
            "  harness_field Client \"\"\"\n\
{{\n\
    let conf = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;\n\
    let conf = {rust_crate}::config::Builder::from(&conf);\n\
    let conf = if let Ok(ep) = std::env::var(\"AWS_ENDPOINT_URL\")\n\
        .or_else(|_| std::env::var(\"DYNAMO_ENDPOINT\"))\n\
    {{\n\
        conf.endpoint_url(ep).build()\n\
    }} else {{\n\
        conf.build()\n\
    }};\n\
    {rust_crate}::Client::from_conf(conf)\n\
}}\n\
\"\"\"\n"
        ));
    }
    // Free-fn → struct constructor map: query → Query, with typed_variant query_as
    let free_fn_bases: std::collections::HashMap<String, String> = free_fns
        .keys()
        .filter(|n| !n.ends_with("_as"))
        .map(|n| {
            let struct_name = snake_to_pascal(n);
            (struct_name, n.clone())
        })
        .collect();

    // Package-level free functions (surface for typecheck / codegen lowering).
    // Emitted as top-level `fn` under the stub root (not under struct/impl) so
    // `use crate as alias` + `alias.fn(...)` / `crate.fn(...)` resolve.
    //
    // Filters:
    // - Skip free fns used as typed_variant bases (`query` when `query_as` exists).
    // - Skip names that already appear as methods/associated fns on a struct —
    //   rustdoc lists those as `function` items without `self` too (e.g. `new`,
    //   `Client::builder`), which must not pollute the package free-fn surface.
    let method_names: std::collections::HashSet<String> = struct_impls
        .values()
        .flat_map(|ms| {
            ms.iter().map(|info| info.name.clone())
        })
        .collect();
    let mut free_fn_names: Vec<_> = free_fns.keys().cloned().collect();
    free_fn_names.sort();
    for name in &free_fn_names {
        if method_names.contains(name) {
            continue;
        }
        if free_fn_bases.values().any(|base| base == name) {
            let typed = format!("{name}_as");
            if free_fns.contains_key(&typed) {
                continue; // constructor free-fn, surfaced on the struct
            }
        }
        if let Some(info) = free_fns.get(name) {
            emit_method_with_template(&mut out, info, None, &rust_crate, 2);
        }
    }

    // Emit structs with their methods
    let default_types_module = if crate_name.starts_with("aws_sdk_") || crate_name.starts_with("aws-sdk-")
    {
        Some("types")
    } else {
        None
    };
    for (_, name) in &struct_ids {
        out.push_str(&format!("\n  struct {}\n", name));
        if let Some(mp) = struct_module_paths.get(name) {
            if !mp.is_empty() && default_types_module != Some(mp.as_str()) {
                out.push_str(&format!("    path {mp}\n"));
            }
        }
        // Typed free-fn constructor only when both free fns exist (query + query_as).
        // Avoids mapping arbitrary free fns (e.g. map) onto structs (Map).
        if let Some(base_fn) = free_fn_bases.get(name) {
            let typed_fn = format!("{base_fn}_as");
            if free_fns.contains_key(&typed_fn) {
                out.push_str(&format!("    typed_variant {typed_fn}\n"));
                out.push_str("    typed_type_params _, return_type\n");
                let already_has_new = struct_impls
                    .get(name)
                    .map(|ms| ms.iter().any(|m| m.name == "new"))
                    .unwrap_or(false);
                if !already_has_new {
                    if let Some(info) = free_fns.get(base_fn) {
                        if let Some(params) = info.sig
                            .strip_prefix(&format!("fn {base_fn}("))
                            .and_then(|s| s.split(')').next())
                        {
                            // Synthesized `new` constructor from free-fn — emit with template
                            let new_info = MethodSigInfo {
                                sig: format!("fn new({params}) -> Self"),
                                name: "new".to_string(),
                                is_async: info.is_async,
                                is_fallible: info.is_fallible,
                                has_self: false,
                                param_names: info.param_names.clone(),
                            };
                            emit_method_with_template(&mut out, &new_info, Some(name), &rust_crate, 4);
                        } else {
                            out.push_str("    fn new(sql: Str) -> Self\n");
                            out.push_str("      lowers_to\n");
                            out.push_str(&format!("        rust: \"{rust_crate}::{name}::new({{{{sql}}}})\"\n"));
                        }
                    }
                }
            }
        }
        if let Some(methods) = struct_impls.get(name) {
            for info in methods {
                emit_method_with_template(&mut out, info, Some(name), &rust_crate, 4);
            }
        }
    }

    // Emit enums with their variants and methods
    for (name, variants) in &enum_defs {
        out.push_str(&format!("\n  enum {}\n", name));
        for v in variants {
            out.push_str(&format!("    {}\n", v));
        }
        // Enum methods (from impl blocks)
        if let Some(methods) = struct_impls.get(name) {
            out.push('\n');
            for info in methods {
                emit_method_with_template(&mut out, info, Some(name), &rust_crate, 4);
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
            let mut methods: Vec<MethodSigInfo> = Vec::new();
            if let Some(items) = items {
                for method_id in items {
                    let method_id_str = method_id.as_u64().map(|n| n.to_string())
                        .or_else(|| method_id.as_str().map(|s| s.to_string()));
                    if let Some(mid) = method_id_str {
                        if let Some(method) = index.get(&mid) {
                            // Only include function items (skip associated types, consts)
                            if let Some(method_inner) = method.get("inner").and_then(|v| v.as_object()) {
                                if method_inner.contains_key("function") {
                                    if let Some(info) = extract_method_sig_with_info(method) {
                                        methods.push(info);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !methods.is_empty() {
                out.push_str(&format!("\n  trait {}\n", name));
                for info in &methods {
                    emit_method_with_template(&mut out, info, Some(name), &rust_crate, 4);
                }
            }
        }
    }

    Ok(out)
}

fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect()
}

/// True when a rustdoc function item is a method (has a `self` receiver).
fn function_item_has_self(item: &serde_json::Value) -> bool {
    let Some(func) = item
        .get("inner")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("function"))
        .and_then(|v| v.as_object())
    else {
        return false;
    };
    let Some(inputs) = func
        .get("sig")
        .and_then(|v| v.as_object())
        .and_then(|s| s.get("inputs"))
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    inputs.iter().any(|input| {
        input
            .as_array()
            .and_then(|a| a.first())
            .and_then(|n| n.as_str())
            == Some("self")
    })
}

/// Metadata extracted from a rustdoc method item for template generation.
struct MethodSigInfo {
    /// The VEIL signature string (e.g. "fn send() -> BoxFuture<Res!<Response>>")
    sig: String,
    /// Method name
    name: String,
    /// Whether the original Rust function is async
    is_async: bool,
    /// Whether the return type is Result/Res!
    is_fallible: bool,
    /// Whether the method takes `self` (instance method vs static/constructor)
    has_self: bool,
    /// Parameter names (excluding self), for template variable interpolation
    param_names: Vec<String>,
}

/// Extract a method signature plus metadata from a rustdoc JSON item.
fn extract_method_sig_with_info(item: &serde_json::Value) -> Option<MethodSigInfo> {
    let name = item.get("name")?.as_str()?;
    let inner = item.get("inner")?.as_object()?;
    let func = inner.get("function")?.as_object()?;
    let sig = func.get("sig")?.as_object()?;

    // Build params
    let inputs = sig.get("inputs").and_then(|v| v.as_array())?;
    let mut has_self = false;
    let mut param_names: Vec<String> = Vec::new();
    let params: Vec<String> = inputs.iter().filter_map(|input| {
        let arr = input.as_array()?;
        let param_name = arr.get(0)?.as_str()?;
        if param_name == "self" { has_self = true; return None; }
        let type_val = arr.get(1)?;
        let type_str = rustdoc_type_to_veil(type_val);
        param_names.push(param_name.to_string());
        Some(format!("{}: {}", param_name, type_str))
    }).collect();

    // Build return type (unwrap impl Future / mark async as BoxFuture for codegen).
    let is_async = func
        .get("header")
        .and_then(|h| h.as_object())
        .and_then(|h| h.get("is_async"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let output = sig.get("output");
    let raw_ret = output.and_then(|o| {
        if o.is_null() {
            return None;
        }
        let t = rustdoc_type_to_veil(o);
        if t == "()" {
            None
        } else {
            Some(t)
        }
    });
    // Detect async: either from `async fn` header OR from return type being
    // `impl Future` / `BoxFuture` (which rustdoc_type_to_veil maps to BoxFuture<...>).
    let is_async = is_async || raw_ret.as_ref().map(|t| t.starts_with("BoxFuture<") || t == "BoxFuture").unwrap_or(false);
    // Determine fallibility from the raw return type (before BoxFuture wrapping).
    // For BoxFuture<Res!<T>>, the inner type is Res!, so it IS fallible.
    let is_fallible = raw_ret.as_ref().map(|t| {
        t.starts_with("Res!") || t.starts_with("Res!<") || t.contains("Res!")
    }).unwrap_or(false);

    let mut ret = raw_ret;
    // `async fn` → BoxFuture so codegen applies `.await.map_err…?`
    // But don't double-wrap: if the return type already IS BoxFuture (from impl Future),
    // leave it alone.
    if is_async && !ret.as_ref().map(|t| t.starts_with("BoxFuture")).unwrap_or(false) {
        ret = Some(match ret {
            Some(inner) => format!("BoxFuture<{inner}>"),
            None => "BoxFuture<()>".into(),
        });
    }

    let sig_str = if let Some(ref ret) = ret {
        format!("fn {}({}) -> {}", name, params.join(", "), ret)
    } else {
        format!("fn {}({})", name, params.join(", "))
    };
    Some(MethodSigInfo {
        sig: sig_str,
        name: name.to_string(),
        is_async,
        is_fallible,
        has_self,
        param_names,
    })
}

/// Generate a `lowers_to` Rust template for a method based on its metadata.
///
/// Template variables:
/// - `{{self}}` — the receiver expression
/// - `{{args}}` — all arguments comma-separated
/// - `{{param_name}}` — individual argument by name
/// - `{{error_model.external}}` — resolved from layer error model at codegen time
///
/// For static/constructor methods (no self), uses `{QualifiedPath}::{method}(...)`.
fn generate_lowers_to_template(
    info: &MethodSigInfo,
    struct_name: Option<&str>,
    rust_crate: &str,
) -> String {
    let method = &info.name;

    // Build the args portion: use named params for positional clarity
    let args_str = if info.param_names.is_empty() {
        "{{args}}".to_string()
    } else {
        info.param_names
            .iter()
            .map(|n| format!("{{{{{n}}}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    if !info.has_self {
        // Static/constructor: qualified path
        let qualified = if let Some(sname) = struct_name {
            format!("{rust_crate}::{sname}")
        } else {
            rust_crate.to_string()
        };
        if info.is_async && info.is_fallible {
            format!(
                "{qualified}::{method}({args_str}).await.map_err(|e| {{{{error_model.external}}}}(format!(\"{{e:?}}\")))?",
            )
        } else if info.is_async {
            format!("{qualified}::{method}({args_str}).await")
        } else if info.is_fallible {
            format!(
                "{qualified}::{method}({args_str}).map_err(|e| {{{{error_model.external}}}}(format!(\"{{e:?}}\")))?",
            )
        } else {
            format!("{qualified}::{method}({args_str})")
        }
    } else {
        // Instance method with self
        if info.is_async && info.is_fallible {
            format!(
                "{{{{self}}}}.{method}({args_str}).await.map_err(|e| {{{{error_model.external}}}}(format!(\"{{e:?}}\")))?",
            )
        } else if info.is_async {
            format!("{{{{self}}}}.{method}({args_str}).await")
        } else if info.is_fallible {
            format!(
                "{{{{self}}}}.{method}({args_str}).map_err(|e| {{{{error_model.external}}}}(format!(\"{{e:?}}\")))?",
            )
        } else {
            format!("{{{{self}}}}.{method}({args_str})")
        }
    }
}

/// Emit a method signature line followed by its `lowers_to` block.
/// `base_indent` is the number of spaces for the `fn` line (usually 4 for struct methods).
fn emit_method_with_template(
    out: &mut String,
    info: &MethodSigInfo,
    struct_name: Option<&str>,
    rust_crate: &str,
    base_indent: usize,
) {
    let indent = " ".repeat(base_indent);
    let child_indent = " ".repeat(base_indent + 2);
    let template = generate_lowers_to_template(info, struct_name, rust_crate);
    out.push_str(&format!("{indent}{}\n", info.sig));
    out.push_str(&format!("{child_indent}lowers_to\n"));
    out.push_str(&format!("{child_indent}  rust: \"{template}\"\n"));
}

/// Convert a rustdoc JSON type representation to VEIL type syntax.
///
/// Handles both rustdoc encodings:
/// - externally tagged: `{ "slice": T }`, `{ "borrowed_ref": { "type": T } }`
/// - internally tagged (format 28+): `{ "kind": "slice", "inner": T }`
///
/// Unknown shapes are `Unknown`, never `Str` — a silent Str fallback is how
/// `QueryOutput.items() -> &[HashMap<…>]` became `fn items() -> Str`.
fn rustdoc_type_to_veil(ty: &serde_json::Value) -> String {
    rustdoc_type_to_veil_depth(ty, 0)
}

fn rustdoc_type_to_veil_depth(ty: &serde_json::Value, depth: u32) -> String {
    if depth > 24 {
        return "Unknown".into();
    }
    let Some(obj) = ty.as_object() else {
        if let Some(s) = ty.as_str() {
            return rustdoc_primitive_or_path(s).to_string();
        }
        return "Unknown".into();
    };

    let (kind, payload) = rustdoc_untag_type(obj);

    match kind.as_deref() {
        Some("resolved_path") => rustdoc_resolved_path(payload, depth),
        Some("primitive") => rustdoc_primitive_payload(payload),
        Some("generic") => payload
            .as_str()
            .unwrap_or("T")
            .to_string(),
        Some("borrowed_ref") => {
            let inner = payload
                .as_object()
                .and_then(|o| o.get("type").or_else(|| o.get("type_")))
                .unwrap_or(payload);
            rustdoc_type_to_veil_depth(inner, depth + 1)
        }
        Some("raw_pointer") => {
            let inner = payload
                .as_object()
                .and_then(|o| o.get("type").or_else(|| o.get("type_")))
                .unwrap_or(payload);
            rustdoc_type_to_veil_depth(inner, depth + 1)
        }
        Some("slice") => {
            format!("List<{}>", rustdoc_type_to_veil_depth(payload, depth + 1))
        }
        Some("array") => {
            let inner = payload
                .as_object()
                .and_then(|o| o.get("type").or_else(|| o.get("type_")))
                .unwrap_or(payload);
            format!("List<{}>", rustdoc_type_to_veil_depth(inner, depth + 1))
        }
        Some("tuple") => {
            let parts = payload
                .as_array()
                .map(|xs| {
                    xs.iter()
                        .map(|t| rustdoc_type_to_veil_depth(t, depth + 1))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if parts.is_empty() {
                "()".into()
            } else {
                format!("({parts})")
            }
        }
        Some("qualified_path") => rustdoc_qualified_path(payload, depth),
        Some("impl_trait") => rustdoc_impl_trait(payload, depth),
        Some("dyn_trait") => "Unknown".into(),
        Some("function_pointer") | Some("infer") | Some("pat") => "Unknown".into(),
        _ => {
            // Last-ditch: old keys that rustdoc_untag missed.
            if let Some(path) = obj.get("resolved_path") {
                return rustdoc_resolved_path(path, depth);
            }
            "Unknown".into()
        }
    }
}

/// Split `{kind, inner}` or `{resolved_path: …}` into (kind, payload).
fn rustdoc_untag_type(obj: &serde_json::Map<String, serde_json::Value>) -> (Option<String>, &serde_json::Value) {
    if let Some(kind) = obj.get("kind").and_then(|k| k.as_str()) {
        let inner = obj.get("inner").unwrap_or(&serde_json::Value::Null);
        return (Some(kind.to_string()), inner);
    }
    const KEYS: &[&str] = &[
        "resolved_path",
        "primitive",
        "generic",
        "borrowed_ref",
        "raw_pointer",
        "slice",
        "array",
        "tuple",
        "impl_trait",
        "dyn_trait",
        "qualified_path",
        "function_pointer",
        "infer",
        "pat",
    ];
    for key in KEYS {
        if let Some(v) = obj.get(*key) {
            return (Some((*key).to_string()), v);
        }
    }
    (None, &serde_json::Value::Null)
}

/// Module under the crate for a rustdoc item (`["aws_sdk_lambda","primitives","Blob"]` → `primitives`).
/// Crate-root types (`Client`) return None so they stay on `root_types`.
fn rustdoc_module_under_crate(
    paths: Option<&serde_json::Map<String, serde_json::Value>>,
    item_id: &str,
    rust_crate: &str,
    type_name: &str,
) -> Option<String> {
    let entry = paths?.get(item_id)?;
    let segs: Vec<&str> = entry
        .get("path")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();
    if segs.len() < 3 {
        return None;
    }
    let start = if segs[0] == rust_crate
        || segs[0] == rust_crate.replace('_', "-")
        || segs[0].replace('-', "_") == rust_crate
    {
        1
    } else {
        0
    };
    let end = if segs.last().copied() == Some(type_name) {
        segs.len() - 1
    } else {
        segs.len()
    };
    if start >= end {
        return None;
    }
    Some(segs[start..end].join("::"))
}

fn rustdoc_primitive_or_path(s: &str) -> &str {
    match s {
        "String" | "str" => "Str",
        "bool" => "Bool",
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "usize" | "isize" => "Int",
        "f32" | "f64" => "F64",
        "Vec" => "List",
        "Option" => "Opt",
        "Result" => "Res",
        "HashMap" | "BTreeMap" => "Map",
        "HashSet" | "BTreeSet" => "Set",
        other => other,
    }
}

fn rustdoc_primitive_payload(payload: &serde_json::Value) -> String {
    let s = payload
        .as_str()
        .or_else(|| payload.get("name").and_then(|n| n.as_str()))
        .unwrap_or("Unknown");
    rustdoc_primitive_or_path(s).to_string()
}

fn rustdoc_resolved_path(payload: &serde_json::Value, depth: u32) -> String {
    let path = payload
        .as_object()
        .and_then(|o| o.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");
    let simple = path.rsplit("::").next().unwrap_or(path);
    let veil_type = rustdoc_primitive_or_path(simple);

    let arg_strs = rustdoc_generic_type_args(payload, depth);
    if arg_strs.is_empty() {
        return veil_type.to_string();
    }
    if veil_type == "Res" {
        return format!("Res!<{}>", arg_strs.first().cloned().unwrap_or_else(|| "()".into()));
    }
    format!("{}<{}>", veil_type, arg_strs.join(", "))
}

fn rustdoc_generic_type_args(path_obj: &serde_json::Value, depth: u32) -> Vec<String> {
    let args_node = path_obj
        .as_object()
        .and_then(|o| o.get("args"))
        .unwrap_or(path_obj);

    let angle = rustdoc_angle_bracketed(args_node);
    let Some(type_args) = angle.and_then(|a| a.get("args")).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    type_args
        .iter()
        .filter_map(|a| rustdoc_generic_arg_type(a, depth))
        .collect()
}

fn rustdoc_angle_bracketed(args: &serde_json::Value) -> Option<&serde_json::Value> {
    let obj = args.as_object()?;
    if let Some(kind) = obj.get("kind").and_then(|k| k.as_str()) {
        if kind == "angle_bracketed" {
            return obj.get("inner");
        }
    }
    obj.get("angle_bracketed")
}

fn rustdoc_generic_arg_type(arg: &serde_json::Value, depth: u32) -> Option<String> {
    let obj = arg.as_object()?;
    if let Some(kind) = obj.get("kind").and_then(|k| k.as_str()) {
        if kind == "type" {
            return obj.get("inner").map(|t| rustdoc_type_to_veil_depth(t, depth + 1));
        }
        return None;
    }
    obj.get("type")
        .map(|t| rustdoc_type_to_veil_depth(t, depth + 1))
}

fn rustdoc_qualified_path(payload: &serde_json::Value, depth: u32) -> String {
    let name = payload
        .as_object()
        .and_then(|o| o.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("Unknown");
    let self_ty = payload
        .as_object()
        .and_then(|o| o.get("self_type"))
        .map(|t| rustdoc_type_to_veil_depth(t, depth + 1));
    match self_ty {
        Some(s) if !s.is_empty() && s != "Unknown" => format!("{s}::{name}"),
        _ => name.to_string(),
    }
}

fn rustdoc_impl_trait(payload: &serde_json::Value, depth: u32) -> String {
    let bounds = payload.as_array().or_else(|| {
        payload
            .as_object()
            .and_then(|o| o.get("bounds"))
            .and_then(|b| b.as_array())
    });
    let Some(bounds) = bounds else {
        return "Unknown".into();
    };
    for b in bounds {
        let tb = b
            .as_object()
            .and_then(|o| o.get("trait_bound"))
            .and_then(|v| v.as_object())
            .or_else(|| {
                // tagged bound
                b.as_object().and_then(|o| {
                    if o.get("kind").and_then(|k| k.as_str()) == Some("trait_bound") {
                        o.get("inner").and_then(|i| i.as_object())
                    } else {
                        None
                    }
                })
            });
        let Some(tb) = tb else { continue };
        let trait_node = tb.get("trait").unwrap_or(&serde_json::Value::Null);
        let path = trait_node
            .as_object()
            .and_then(|t| t.get("path"))
            .and_then(|p| p.as_str())
            .unwrap_or("");
        if path.ends_with("Future") || path == "Future" {
            if let Some(out) = rustdoc_future_output(trait_node, depth) {
                return format!("BoxFuture<{out}>");
            }
            return "BoxFuture<()>".into();
        }
        // `impl Into<String>` / `impl Into<AttributeValue>` — use the target type.
        if path.ends_with("Into") || path == "Into" {
            let args = rustdoc_generic_type_args(trait_node, depth);
            if let Some(t) = args.first() {
                return t.clone();
            }
        }
        if path.ends_with("AsRef") || path == "AsRef" {
            let args = rustdoc_generic_type_args(trait_node, depth);
            if args.iter().any(|a| a == "Str" || a == "str") {
                return "Str".into();
            }
            if let Some(t) = args.first() {
                return t.clone();
            }
        }
    }
    "Unknown".into()
}

fn rustdoc_future_output(trait_node: &serde_json::Value, depth: u32) -> Option<String> {
    let angle = rustdoc_angle_bracketed(
        trait_node
            .as_object()
            .and_then(|t| t.get("args"))
            .unwrap_or(trait_node),
    )?;
    let constraints = angle.get("constraints")?.as_array()?;
    for c in constraints {
        let binding = c
            .as_object()
            .and_then(|o| o.get("binding"))
            .and_then(|b| b.as_object())
            .or_else(|| {
                c.as_object().and_then(|o| {
                    if o.get("kind").and_then(|k| k.as_str()) == Some("binding") {
                        o.get("inner").and_then(|i| i.as_object())
                    } else {
                        None
                    }
                })
            })?;
        let name = binding.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name != "Output" && !name.is_empty() {
            continue;
        }
        if let Some(ty) = binding
            .get("equality")
            .and_then(|e| e.as_object())
            .and_then(|e| e.get("type"))
            .or_else(|| binding.get("type"))
        {
            return Some(rustdoc_type_to_veil_depth(ty, depth + 1));
        }
    }
    None
}

// ─── Agent Connect (ACP Tunnel Bridge) ──────────────────────────────────────

/// Read runtime URL and token from ~/.veil/config.json.
fn read_veil_config() -> (Option<String>, Option<String>) {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".veil/config.json"))
        .unwrap_or_default();
    if let Ok(contents) = std::fs::read_to_string(&config_path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&contents) {
            let url = val.get("runtime_url").and_then(|v| v.as_str()).map(String::from);
            let token = val.get("token").and_then(|v| v.as_str()).map(String::from);
            return (url, token);
        }
    }
    (None, None)
}

/// `veil agent connect` — spawn a local ACP agent and bridge to the runtime.
async fn run_agent_connect(command: &str, runtime_url: Option<String>, token: Option<String>, agent_name: &str) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command as TokioCommand;

    let (config_url, config_token) = read_veil_config();
    let base_url = runtime_url
        .or(config_url)
        .unwrap_or_else(|| "http://localhost:8080".into());
    let auth_token = token
        .or(config_token)
        .unwrap_or_else(|| "default_user".into());

    // Build WebSocket URL
    let ws_url = {
        let base = base_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        let encoded_name = agent_name.replace(' ', "+");
        format!("{base}/api/agent/acp?token={auth_token}&agent_name={encoded_name}")
    };

    eprintln!("╭─ veil agent connect");
    eprintln!("│  Agent:   {agent_name}");
    eprintln!("│  Command: {command}");
    eprintln!("│  Runtime: {base_url}");
    eprintln!("│  WebSocket: {ws_url}");
    eprintln!("╰─");
    eprintln!();

    // Connect WebSocket to the runtime
    eprintln!("Connecting to runtime...");
    let (ws_stream, _response) = match tokio_tungstenite::connect_async(&ws_url).await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("error: Failed to connect to runtime at {ws_url}: {e}");
            eprintln!();
            eprintln!("Is the runtime running? Try:");
            eprintln!("  cargo run -p veil-runtime");
            std::process::exit(1);
        }
    };
    eprintln!("✓ Connected to runtime");

    let (mut ws_write, mut ws_read) = futures::StreamExt::split(ws_stream);

    // Read the welcome message
    if let Some(Ok(msg)) = futures::StreamExt::next(&mut ws_read).await {
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            if let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) {
                if frame.get("type").and_then(|v| v.as_str()) == Some("connected") {
                    eprintln!("✓ ACP tunnel established");
                }
            }
        }
    }

    // Spawn the local agent process
    eprintln!("Spawning agent: {command}");
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        eprintln!("error: empty command");
        std::process::exit(1);
    }

    let mut child = match TokioCommand::new(parts[0])
        .args(&parts[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: Failed to spawn agent process `{command}`: {e}");
            eprintln!();
            if command.contains("kiro-cli") {
                eprintln!("Is kiro-cli installed? Check: which kiro-cli");
            }
            std::process::exit(1);
        }
    };
    eprintln!("✓ Agent process started (PID: {})", child.id().unwrap_or(0));
    eprintln!();
    eprintln!("Bridge active. Ctrl+C to disconnect.");
    eprintln!("Dashboard should now show: Agent: Connected ({agent_name})");
    eprintln!();

    let mut child_stdin = child.stdin.take().expect("child stdin");
    let child_stdout = child.stdout.take().expect("child stdout");
    let mut stdout_reader = BufReader::new(child_stdout).lines();

    // Bridge: Runtime WS → Agent stdin, Agent stdout → Runtime WS
    loop {
        tokio::select! {
            // Runtime → Agent: forward turn_request and tool_result frames to the process stdin
            ws_msg = futures::StreamExt::next(&mut ws_read) => {
                match ws_msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        // Forward the frame to the agent process stdin (JSON line)
                        let mut line = text.to_string();
                        if !line.ends_with('\n') {
                            line.push('\n');
                        }
                        if child_stdin.write_all(line.as_bytes()).await.is_err() {
                            eprintln!("Agent process stdin closed");
                            break;
                        }
                        let _ = child_stdin.flush().await;
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                        eprintln!("Runtime WebSocket closed");
                        break;
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                        let _ = futures::SinkExt::send(
                            &mut ws_write,
                            tokio_tungstenite::tungstenite::Message::Pong(data)
                        ).await;
                    }
                    Some(Err(e)) => {
                        eprintln!("WebSocket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
            // Agent → Runtime: read JSON lines from stdout and send to WS
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        let text = text.trim().to_string();
                        if text.is_empty() { continue; }
                        // Validate it's JSON before sending
                        if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                            eprintln!("warning: non-JSON line from agent, skipping: {}", &text[..text.len().min(80)]);
                            continue;
                        }
                        if futures::SinkExt::send(
                            &mut ws_write,
                            tokio_tungstenite::tungstenite::Message::Text(text.into())
                        ).await.is_err() {
                            eprintln!("Failed to send to runtime WebSocket");
                            break;
                        }
                    }
                    Ok(None) => {
                        eprintln!("Agent process stdout closed (process exited)");
                        break;
                    }
                    Err(e) => {
                        eprintln!("Error reading agent stdout: {e}");
                        break;
                    }
                }
            }
            // Agent process exit
            status = child.wait() => {
                match status {
                    Ok(s) => eprintln!("Agent process exited with: {s}"),
                    Err(e) => eprintln!("Agent process error: {e}"),
                }
                break;
            }
        }
    }

    // Cleanup
    let _ = child.kill().await;
    let _ = futures::SinkExt::close(&mut ws_write).await;
    eprintln!();
    eprintln!("ACP tunnel disconnected.");
}

/// `veil agent status` — query the runtime for agent provider info.
async fn run_agent_status(runtime_url: Option<String>) {
    let (config_url, _) = read_veil_config();
    let base_url = runtime_url
        .or(config_url)
        .unwrap_or_else(|| "http://localhost:8080".into());

    let url = format!("{base_url}/api/agent/status");
    match reqwest::get(&url).await {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let provider = data.get("provider").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let mode = data.get("provider_mode")
                            .and_then(|v| v.get("mode"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let connected = data.get("acp_tunnel")
                            .and_then(|v| v.get("connected"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);

                        println!("Agent Status");
                        println!("────────────────────────────");
                        println!("  Provider: {provider} ({mode})");
                        println!("  ACP Tunnel: {}", if connected { "connected ✓" } else { "not connected" });

                        if let Some(agents) = data.get("acp_tunnel")
                            .and_then(|v| v.get("agents"))
                            .and_then(|v| v.as_array())
                        {
                            for agent in agents {
                                let name = agent.get("agent_name").and_then(|v| v.as_str()).unwrap_or("?");
                                let uid = agent.get("user_id").and_then(|v| v.as_str()).unwrap_or("?");
                                println!("    → {name} ({uid})");
                            }
                        }

                        if !connected {
                            println!();
                            println!("  Connect with: veil agent connect");
                        }
                    }
                    Err(e) => eprintln!("error: invalid JSON response: {e}"),
                }
            } else {
                eprintln!("error: runtime returned {}", resp.status());
            }
        }
        Err(e) => {
            eprintln!("error: cannot reach runtime at {url}: {e}");
            eprintln!();
            eprintln!("Is the runtime running?");
        }
    }
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
            trace_passes,
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
                    // ACS-008: full structured items (code, severity, message, span, hint)
                    "diagnostics": result
                        .diagnostics
                        .iter()
                        .map(veil_ir::StructuredDiagnostic::from)
                        .collect::<Vec<_>>(),
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

            // --trace-passes: run passes with tracing and print results
            if trace_passes {
                let mut trace_sol = sol.clone();
                let traces = veil_codegen::pass_exec::execute_pre_passes(&mut trace_sol, &registry, true);
                if traces.is_empty() {
                    eprintln!("[trace-passes] No passes matched any nodes.");
                } else {
                    for t in &traces {
                        eprintln!(
                            "[pass:{}/{:?}/{}] {} ({}) → {}",
                            t.pass_name,
                            "pre",
                            registry.passes.iter()
                                .find(|p| p.name == t.pass_name)
                                .map(|p| p.priority)
                                .unwrap_or(0),
                            t.construct_name,
                            t.construct_kind,
                            t.action_desc,
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
        Commands::GenHarness { files, output } => {
            if files.is_empty() {
                eprintln!("error: at least one .veil file required");
                std::process::exit(1);
            }
            // Parse all packages
            let mut packages: Vec<(veil_ir::ast::Solution, veil_ir::LayerRegistry)> = Vec::new();
            for file in &files {
                let source = std::fs::read_to_string(file)
                    .unwrap_or_else(|e| { eprintln!("cannot read {}: {e}", file.display()); std::process::exit(1); });
                let registry = registry_for(file);
                let tokens = veil_parser::lex(&source);
                let veil_file = match veil_parser::parse_file_with_registry(&tokens, registry.clone()) {
                    Ok(f) => f,
                    Err(errors) => {
                        eprintln!("Parse errors in {}:", file.display());
                        for err in &errors { eprintln!("  {}", err); }
                        std::process::exit(1);
                    }
                };
                // Same adapt merge as `veil gen` so product packages that
                // `use application` (etc.) carry stock handlers into the harness.
                let sol = match veil_file {
                    veil_ir::ast::VeilFile::Solution(s) => s,
                    veil_ir::ast::VeilFile::Package(pkg) => {
                        match merge_package_or_exit(&pkg, file) {
                            Ok(s) => s,
                            Err(()) => std::process::exit(1),
                        }
                    }
                    _ => {
                        eprintln!("{}: not a package file", file.display());
                        std::process::exit(1);
                    }
                };
                packages.push((sol, registry));
            }

            let refs: Vec<(&veil_ir::ast::Solution, &veil_ir::LayerRegistry)> =
                packages.iter().map(|(s, r)| (s, r)).collect();
            let harness_files = veil_codegen::generate_multi_harness(&refs);

            for f in &harness_files {
                let path = output.join(&f.path);
                write_file_atomic(&path, f.content.as_bytes()).expect("Failed to write file");
            }

            // Ensure workspace.dependencies includes every stub (and cargo_deps
            // companions) referenced by the multi-package veil_bin — last package
            // gen alone may have left only one package's stubs (e.g. sqlx without
            // aws-config).
            let mut stub_specs: Vec<(String, String, Vec<String>, Vec<(String, String)>)> =
                Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (_, reg) in &packages {
                for stub in &reg.stubs {
                    if !seen.insert(stub.name.clone()) {
                        continue;
                    }
                    stub_specs.push((
                        stub.name.clone(),
                        stub.version.clone(),
                        stub.cargo_features.clone(),
                        stub.cargo_deps.clone(),
                    ));
                }
            }
            let ws_path = output.join("Cargo.toml");
            if ws_path.is_file() && !stub_specs.is_empty() {
                if let Ok(ws) = std::fs::read_to_string(&ws_path) {
                    let patched = merge_stub_workspace_deps(&ws, &stub_specs);
                    if patched != ws {
                        let _ = std::fs::write(&ws_path, patched);
                    }
                }
            }

            // Finalize workspace members so every context crate under crates/
            // is listed (last single-package gen only had its own members).
            finalize_multi_package_workspace(&output);

            // Format
            for f in &harness_files {
                if f.path.ends_with(".rs") {
                    let path = output.join(&f.path);
                    let _ = std::process::Command::new("rustfmt")
                        .args(["--edition", "2024", &path.to_string_lossy()])
                        .output();
                }
            }
            println!("✓ Generated multi-package harness ({} files) in {}", harness_files.len(), output.display());
        }
        Commands::Gen { file, output, target, no_prune, trace_passes } => {
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

            // --trace-passes: run passes with tracing and print results
            if trace_passes {
                let trace_sol = match &veil_file {
                    veil_ir::ast::VeilFile::Solution(sol) => Some(sol.clone()),
                    veil_ir::ast::VeilFile::Package(pkg) => {
                        Some(veil_ir::package_as_solution(pkg))
                    }
                    _ => None,
                };
                if let Some(mut sol) = trace_sol {
                    let traces = veil_codegen::pass_exec::execute_pre_passes(&mut sol, &registry, true);
                    if traces.is_empty() {
                        eprintln!("[trace-passes] No passes matched any nodes.");
                    } else {
                        for t in &traces {
                            eprintln!(
                                "[pass:{}/{:?}/{}] {} ({}) → {}",
                                t.pass_name,
                                "pre",
                                registry.passes.iter()
                                    .find(|p| p.name == t.pass_name)
                                    .map(|p| p.priority)
                                    .unwrap_or(0),
                                t.construct_name,
                                t.construct_kind,
                                t.action_desc,
                            );
                        }
                    }
                }
            }

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
                    // Also inject implicit adapts for `use X` + companion X.veil
                    // (hub siblings + veil.toml [dependencies] — R20).
                    let mut pkg = pkg.clone();
                    let search = veil_ir::adapt_search_paths_for_file(&file);
                    veil_ir::inject_implicit_adapts(&mut pkg, &search);
                    // Expand macros before codegen (AST→AST transform).
                    // Load macros from use'd packages (cross-package macro resolution).
                    let external_macros = load_external_macros(&pkg, &file, &registry);
                    if external_macros.is_empty() {
                        veil_ir::macros::expand_macros(&mut pkg);
                    } else {
                        veil_ir::macros::expand_macros_with(&mut pkg, &external_macros);
                    }
                    let sol = if !pkg.adapts.is_empty() || !pkg.patches.is_empty() {
                        match merge_package_or_exit(&pkg, &file) {
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
                            guidance: Vec::new(),
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
                                    veil_codegen::ts::api_client::generate_api_client_from_package(&pkg);
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

            let mut written_count = 0;
            for f in &files {
                let path = output.join(&f.path);
                if write_file_atomic(&path, f.content.as_bytes()).expect("Failed to write file") {
                    written_count += 1;
                }
            }

            // Single-file gen must not leave crates from a prior multi-package
            // or different package gen (e.g. stale `crates/iaaa` after removing
            // example_core). Prune crates/ entries not present in this gen's files.
            // Multi-package devloop passes --no-prune so sibling packages survive
            // intermediate gens; it prunes once after gen-harness.
            if !no_prune {
                prune_stale_gen_crates(&output, &files);
            }

            // Run formatters on generated files (spikes skip formatters)
            if written_count > 0 {
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
                    // Skip prettier — gen output is stable and running prettier
                    // would cause content drift that defeats the skip-unchanged logic.
                }
                veil_codegen::CodegenTarget::Swift | veil_codegen::CodegenTarget::Kotlin => {}
            }
            } // end if written_count > 0

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
        Commands::Agent { action } => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                match action {
                    AgentCmd::Connect { command, runtime_url, token, agent_name } => {
                        run_agent_connect(&command, runtime_url, token, &agent_name).await;
                    }
                    AgentCmd::Status { runtime_url } => {
                        run_agent_status(runtime_url).await;
                    }
                }
            });
        }
        Commands::StubGen {
            crate_name,
            output,
            project,
            features,
        } => {
            let output_path = output.unwrap_or_else(|| PathBuf::from(format!("{}.stub", crate_name)));
            match generate_stub(&crate_name, &project, &features) {
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
        Commands::New { what } => {
            match what {
                NewCmd::Extension { name, kind, dir } => {
                    let target_dir = dir.unwrap_or_else(|| PathBuf::from(".")).join(&name);
                    if target_dir.exists() {
                        eprintln!("error: directory '{}' already exists", target_dir.display());
                        std::process::exit(1);
                    }
                    std::fs::create_dir_all(&target_dir).expect("create extension dir");
                    
                    // Generate veil.toml
                    let veil_toml = format!(
                        r#"[package]
name = "{name}"
version = "0.1.0"
kind = "{kind}"

[extension]
scope = "Product"
provenance = "Custom"

[build]
target = "rust"
"#,
                        name = name,
                        kind = kind,
                    );
                    std::fs::write(target_dir.join("veil.toml"), &veil_toml).expect("write veil.toml");

                    // Generate main.veil
                    let kind_pascal = kind.split('-').map(|s| {
                        let mut c = s.chars();
                        match c.next() {
                            None => String::new(),
                            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        }
                    }).collect::<String>();
                    let pkg_name = name.replace('-', "_");
                    let main_veil = format!(
                        r#"pkg {pkg_name}

  struct {kind_pascal}Input
    trigger: Str
    payload: Json

  struct {kind_pascal}Output
    success: Bool
    result: Json

  fn handle_{pkg_name}
    -> {kind_pascal}Output
    input
      request: {kind_pascal}Input
    step execute
      # TODO: implement extension logic
      ret {kind_pascal}Output{{success: true, result: {{handled: request.trigger}}}}
"#,
                        pkg_name = pkg_name,
                        kind_pascal = kind_pascal,
                    );
                    std::fs::write(target_dir.join("main.veil"), &main_veil).expect("write main.veil");

                    // Generate README
                    let readme = format!(
                        "# {name}\n\nA VEIL extension ({kind}).\n\n## Development\n\n```bash\nveil check main.veil\nveil gen main.veil -o generated -t rust\n```\n\n## Deploy\n\n```bash\nveil deploy --env dev\n```\n",
                        name = name,
                        kind = kind,
                    );
                    std::fs::write(target_dir.join("README.md"), &readme).expect("write README");

                    println!("✓ Created extension '{}' at {}", name, target_dir.display());
                    println!("  Kind: {}", kind);
                    println!("  Files:");
                    println!("    veil.toml    — package config");
                    println!("    main.veil    — extension source");
                    println!("    README.md    — documentation");
                    println!();
                    println!("  Next steps:");
                    println!("    cd {}", name);
                    println!("    veil check main.veil");
                    println!("    # Edit main.veil to implement your extension logic");
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
                    println!("  veil check {}/main.veil", info.path);
                    println!("  edit {}/MISSION.md  # product intent for the coding agent", info.path);
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
                println!("  Viewer:       http://localhost:{port}/viewer/?project=<name>&mode=reaction");
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
                                guidance: Vec::new(),
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
        Commands::Test {
            file,
            filter,
            unit,
            component,
            scenarios,
            integration,
            target,
            json,
            watch,
            coverage,
            update_snapshots,
        } => {
            // Determine which file(s) to process.
            let files: Vec<PathBuf> = if scenarios {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                collect_test_veil_files(&cwd)
            } else if let Some(f) = file.clone() {
                vec![f]
            } else {
                eprintln!("error: provide a .veil file or use --scenarios");
                std::process::exit(1);
            };

            if files.is_empty() {
                eprintln!("No test files found.");
                std::process::exit(1);
            }

            let target_str = target.as_deref().unwrap_or("rust");
            let codegen_target = veil_codegen::CodegenTarget::from_str(target_str)
                .unwrap_or_else(|| {
                    eprintln!("Unknown target '{}'. Use: rust, typescript", target_str);
                    std::process::exit(2);
                });

            // ─── Watch mode ─────────────────────────────────────────────
            if watch {
                run_test_watch_mode(
                    &files, &filter, unit, component, scenarios, integration,
                    codegen_target, target_str, json, coverage, update_snapshots,
                );
                return;
            }

            // ─── Single run ─────────────────────────────────────────────
            let exit_code = run_tests_once(
                &files, &filter, unit, component, scenarios, integration,
                codegen_target, target_str, json, coverage, update_snapshots,
            );
            std::process::exit(exit_code);
        }
    }
}

/// Run the test pipeline once. Returns the process exit code.
fn run_tests_once(
    files: &[PathBuf],
    filter: &Option<String>,
    unit: bool,
    component: bool,
    scenarios: bool,
    integration: bool,
    codegen_target: veil_codegen::CodegenTarget,
    target_str: &str,
    json: bool,
    coverage: bool,
    update_snapshots: bool,
) -> i32 {
    let mut all_items: Vec<veil_ir::TopLevelItem> = Vec::new();
    let mut all_solutions: Vec<veil_ir::Solution> = Vec::new();
    let mut all_registries: Vec<LayerRegistry> = Vec::new();
    let mut total_tests = 0usize;
    let mut total_filtered = 0usize;

    for path in files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", path.display(), e);
                return 1;
            }
        };
        let (sol, registry) = parse_solution_or_exit(&source, path);

        // Collect test blocks and filter.
        let mut test_cases: Vec<(&str, &veil_ir::TestCase)> = Vec::new();
        for item in &sol.items {
            match item {
                veil_ir::TopLevelItem::TestBlock(tb) => {
                    for case in &tb.cases {
                        let is_component = case.mount.is_some();
                        let is_unit = !is_component;
                        if unit && !is_unit { continue; }
                        if component && !is_component { continue; }
                        if let Some(f) = filter {
                            if !case.name.contains(f.as_str()) { continue; }
                        }
                        test_cases.push((
                            tb.target.as_deref().unwrap_or("anonymous"),
                            case,
                        ));
                    }
                }
                veil_ir::TopLevelItem::Integration(integ) => {
                    if integration || (!unit && !component && !scenarios) {
                        if let Some(f) = filter {
                            if !integ.name.contains(f.as_str()) { continue; }
                        }
                        total_tests += 1;
                        total_filtered += 1;
                        if !json {
                            println!("  integration: {}", integ.name);
                        }
                    }
                }
                _ => {}
            }
        }

        total_tests += test_cases.len();
        total_filtered += test_cases.len();

        if !json && !coverage {
            println!("{}:", path.display());
            for (target_name, case) in &test_cases {
                let kind = if case.mount.is_some() { "component" } else { "unit" };
                println!("  {} [{}] {}", target_name, kind, case.name);
            }
        }

        all_items.extend(sol.items.clone());
        all_solutions.push(sol);
        all_registries.push(registry);
    }

    // ─── Coverage report ────────────────────────────────────────────────
    if coverage {
        for sol in &all_solutions {
            let report = veil_ir::compute_coverage(sol);
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            } else {
                println!("Coverage Report");
                println!("───────────────────────────────────────");
                println!(
                    "  Functions: {}/{} ({:.1}%)",
                    report.functions.covered, report.functions.total, report.functions.percent
                );
                println!(
                    "  Branches:  {}/{} ({:.1}%)",
                    report.branches.covered, report.branches.total, report.branches.percent
                );
                println!(
                    "  Nodes:     {}/{} ({:.1}%)",
                    report.nodes.covered, report.nodes.total, report.nodes.percent
                );
                if !report.uncovered.is_empty() {
                    println!();
                    println!("  Uncovered:");
                    for item in &report.uncovered {
                        println!("    [{}] {} (line {})", item.kind, item.name, item.line);
                    }
                }
                println!();
            }
        }
        return 0;
    }

    // ─── Codegen + runner invocation ────────────────────────────────────
    if total_tests == 0 {
        if !json {
            println!("\nNo test code generated (0 test blocks found).");
        }
        return 0;
    }

    // Write generated files to a temp directory.
    let tmp_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot create temp dir: {}", e);
            return 1;
        }
    };
    let tmp_path = tmp_dir.path();

    match codegen_target {
        veil_codegen::CodegenTarget::Rust => {
            // SL-022: emit the real product crate (tests.rs calls handlers).
            let sol = &all_solutions[0];
            let registry = &all_registries[0];
            let project = veil_codegen::generate(sol, registry);
            for gen_file in &project.files {
                let dest = tmp_path.join(&gen_file.path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                if let Err(e) = std::fs::write(&dest, &gen_file.content) {
                    eprintln!("error: write {}: {e}", dest.display());
                    return 1;
                }
            }

            // Invoke cargo test
            let mut cmd = std::process::Command::new("cargo");
            cmd.arg("test").arg("--lib").current_dir(tmp_path);
            if let Some(f) = filter {
                cmd.arg("--").arg(f);
            }
            if update_snapshots {
                cmd.env("INSTA_UPDATE", "always");
            }
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());

            if json {
                let report = serde_json::json!({
                    "status": "running",
                    "target": target_str,
                    "files": files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                    "total_tests": total_tests,
                    "filtered_tests": total_filtered,
                    "runner": "cargo test",
                    "temp_dir": tmp_path.display().to_string(),
                });
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            }

            match cmd.status() {
                Ok(status) => status.code().unwrap_or(1),
                Err(e) => {
                    eprintln!("error: failed to run cargo test: {}", e);
                    1
                }
            }
        }
        veil_codegen::CodegenTarget::TypeScript => {
            let test_files = veil_codegen::testing::generate_ts_tests(&all_items);
            // Write package.json + vitest config
            let package_json = r#"{"name":"veil-tests","private":true,"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"^1"}}"#;
            let vitest_config = "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { globals: true } });\n";

            std::fs::create_dir_all(tmp_path.join("src/__tests__")).ok();
            std::fs::write(tmp_path.join("package.json"), package_json).ok();
            std::fs::write(tmp_path.join("vitest.config.ts"), vitest_config).ok();

            for gen_file in &test_files {
                let dest = tmp_path.join(&gen_file.path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&dest, &gen_file.content).ok();
            }

            // Check if there's a scenario file (playwright)
            let has_scenarios = test_files.iter().any(|f| f.path.contains("e2e/"));

            if has_scenarios && scenarios {
                // Run Playwright for scenarios
                let mut cmd = std::process::Command::new("npx");
                cmd.arg("playwright").arg("test").current_dir(tmp_path);
                if update_snapshots {
                    cmd.arg("--update-snapshots");
                }
                cmd.stdout(std::process::Stdio::inherit());
                cmd.stderr(std::process::Stdio::inherit());

                if json {
                    let report = serde_json::json!({
                        "status": "running",
                        "target": target_str,
                        "runner": "playwright test",
                        "temp_dir": tmp_path.display().to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&report).unwrap());
                }

                match cmd.status() {
                    Ok(status) => status.code().unwrap_or(1),
                    Err(e) => {
                        eprintln!("error: failed to run playwright test: {}", e);
                        1
                    }
                }
            } else {
                // Run vitest for unit/component tests
                let mut cmd = std::process::Command::new("npx");
                cmd.arg("vitest").arg("run").current_dir(tmp_path);
                if let Some(f) = filter {
                    cmd.arg("--filter").arg(f);
                }
                if update_snapshots {
                    cmd.arg("--update");
                }
                cmd.stdout(std::process::Stdio::inherit());
                cmd.stderr(std::process::Stdio::inherit());

                if json {
                    let report = serde_json::json!({
                        "status": "running",
                        "target": target_str,
                        "runner": "vitest",
                        "temp_dir": tmp_path.display().to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&report).unwrap());
                }

                match cmd.status() {
                    Ok(status) => status.code().unwrap_or(1),
                    Err(e) => {
                        eprintln!("error: failed to run vitest: {}", e);
                        1
                    }
                }
            }
        }
        _ => {
            eprintln!("Unsupported runner for target: {}", target_str);
            2
        }
    }
}

/// Watch mode: use notify to watch .veil/.test.veil files and re-run tests on changes.
fn run_test_watch_mode(
    files: &[PathBuf],
    filter: &Option<String>,
    unit: bool,
    component: bool,
    scenarios: bool,
    integration: bool,
    codegen_target: veil_codegen::CodegenTarget,
    target_str: &str,
    json: bool,
    coverage: bool,
    update_snapshots: bool,
) {
    use notify::{Event, RecursiveMode, Watcher};

    // Determine the watch root — parent of the first file, or cwd for scenarios.
    let watch_root = if scenarios {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        files[0]
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };

    println!("Watch mode: monitoring {} for .veil changes", watch_root.display());
    println!("Press Ctrl+C to stop.\n");

    // Initial run.
    let _ = run_tests_once(
        files, filter, unit, component, scenarios, integration,
        codegen_target, target_str, json, coverage, update_snapshots,
    );

    let (notify_tx, notify_rx) = std::sync::mpsc::channel();
    let mut watcher = match notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            let _ = notify_tx.send(event);
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: failed to create file watcher: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = watcher.watch(&watch_root, RecursiveMode::Recursive) {
        eprintln!("error: failed to watch {}: {}", watch_root.display(), e);
        std::process::exit(1);
    }

    // Set up Ctrl+C handler.
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc_flag(&r);

    let debounce_ms = std::time::Duration::from_millis(300);
    let mut last_run = std::time::Instant::now() - debounce_ms;

    while running.load(std::sync::atomic::Ordering::Relaxed) {
        match notify_rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(event) => {
                // Filter: only .veil files.
                let relevant = event.paths.iter().any(|p| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    name.ends_with(".veil")
                });
                if !relevant {
                    continue;
                }

                // Debounce.
                let now = std::time::Instant::now();
                if now.duration_since(last_run) < debounce_ms {
                    continue;
                }
                last_run = now;

                // Clear screen and re-run.
                print!("\x1b[2J\x1b[H");
                println!("─── File changed: {:?} ───\n", event.paths);

                // Re-collect files for scenarios mode.
                let run_files: Vec<PathBuf> = if scenarios {
                    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    collect_test_veil_files(&cwd)
                } else {
                    files.to_vec()
                };

                let _ = run_tests_once(
                    &run_files, filter, unit, component, scenarios, integration,
                    codegen_target, target_str, json, coverage, update_snapshots,
                );

                println!("\n─── Waiting for changes... ───");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("\nWatch mode stopped.");
}

/// Set up a Ctrl+C handler that sets an atomic bool to false.
fn ctrlc_flag(running: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    // On Unix, rely on the default SIGINT behavior (terminates process).
    // The AtomicBool flag provides a cooperative shutdown path for the
    // watch loop — if the watcher channel disconnects, the loop exits too.
    let _ = running;
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

/// Recursively collect `*.test.veil` files for `--scenarios` mode.
fn collect_test_veil_files(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(collect_test_veil_files(&path));
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".test.veil") {
                    results.push(path);
                }
            }
        }
    }
    results
}
