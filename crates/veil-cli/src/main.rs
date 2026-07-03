//! VEIL CLI — parse, validate, generate, and serve VEIL files.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    /// Validate a VEIL file
    Check {
        /// Path to the .veil file
        file: PathBuf,
    },
    /// Generate Rust code from a VEIL file
    Gen {
        /// Path to the .veil file
        file: PathBuf,
        /// Output directory
        #[arg(short, long, default_value = "./output")]
        output: PathBuf,
    },
    /// Start the visualization server
    Serve {
        /// Path to the .veil file
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
}

/// Palette construct definition for the viewer
#[derive(serde::Serialize)]
struct PaletteConstruct {
    name: String,
    kind: String,
    icon: String,
    color: String,
    label: String,
    group: String,
    allowed_in: String,
}

/// Build the palette configuration by reading only .layer files referenced by the .veil file.
fn build_palette_config(file_path: &std::path::Path) -> Vec<PaletteConstruct> {
    let mut constructs = Vec::new();
    let dir = file_path.parent().unwrap_or(std::path::Path::new("."));

    // Read the .veil file to find 'use' declarations
    let veil_content = std::fs::read_to_string(file_path).unwrap_or_default();
    let used_layers: Vec<String> = veil_content.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("use "))
        .map(|l| l.strip_prefix("use ").unwrap_or("").trim().to_string())
        .collect();

    // Load only the referenced .layer files
    for layer_name in &used_layers {
        let layer_path = dir.join(format!("{}.layer", layer_name));
        if let Ok(content) = std::fs::read_to_string(&layer_path) {
            constructs.extend(parse_layer_constructs(&content));
        }
    }

    // If no layers referenced, return empty (base primitives could be implicit later)
    constructs
}

/// Parse construct definitions from a .layer file content.
fn parse_layer_constructs(content: &str) -> Vec<PaletteConstruct> {
    let mut constructs = Vec::new();
    let mut current: Option<PaletteConstruct> = None;
    let mut in_visual = false;
    let mut in_skip_block = false;

    for line in content.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        if trimmed.starts_with("construct ") {
            if let Some(c) = current.take() { constructs.push(c); }
            let name = trimmed.strip_prefix("construct ").unwrap().trim().to_string();
            current = Some(PaletteConstruct {
                name: name.clone(), kind: String::new(), icon: String::new(),
                color: String::new(), label: name, group: String::new(), allowed_in: String::new(),
            });
            in_visual = false;
            in_skip_block = false;
        } else if trimmed.starts_with("statement ") {
            if let Some(c) = current.take() { constructs.push(c); }
            current = None;
            in_visual = false;
            in_skip_block = false;
        } else if let Some(ref mut c) = current {
            if (trimmed == "contains" || trimmed == "constraints") && !in_visual {
                in_skip_block = true;
            } else if trimmed == "visual" {
                in_visual = true;
                in_skip_block = false;
            } else if in_skip_block {
                if indent <= 4 && !trimmed.is_empty() && !trimmed.starts_with('#') {
                    in_skip_block = false;
                    if trimmed == "visual" {
                        in_visual = true;
                    } else if trimmed == "contains" || trimmed == "constraints" {
                        in_skip_block = true;
                    } else {
                        parse_construct_field(c, trimmed);
                    }
                }
            } else if in_visual {
                if trimmed.starts_with("icon ") {
                    c.icon = extract_quoted(trimmed.strip_prefix("icon ").unwrap_or(""));
                } else if trimmed.starts_with("color ") {
                    c.color = extract_quoted(trimmed.strip_prefix("color ").unwrap_or(""));
                } else if trimmed.starts_with("label ") {
                    c.label = extract_quoted(trimmed.strip_prefix("label ").unwrap_or(""));
                } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    in_visual = false;
                    parse_construct_field(c, trimmed);
                }
            } else if !trimmed.is_empty() {
                parse_construct_field(c, trimmed);
            }
        }
    }
    if let Some(c) = current.take() { constructs.push(c); }

    for c in &mut constructs {
        if c.kind.is_empty() {
            c.kind = match c.name.as_str() {
                "Context" | "Orchestrator" => "Module".to_string(),
                "Aggregate" | "Entity" | "ValueObject" | "Event" | "Command" => "TypeDef".to_string(),
                "Port" | "Repository" => "Interface".to_string(),
                "Adapter" => "Implementation".to_string(),
                "DomainService" | "Saga" => "Flow".to_string(),
                _ => "TypeDef".to_string(),
            };
        }
    }
    constructs
}

fn parse_construct_field(c: &mut PaletteConstruct, line: &str) {
    if line.starts_with("maps_to ") {
        let val = line.strip_prefix("maps_to ").unwrap_or("").trim();
        c.kind = match val {
            "mod" => "Module".to_string(),
            "struct" => "TypeDef".to_string(),
            "trait" => "Interface".to_string(),
            "impl" => "Implementation".to_string(),
            "fn" => "Flow".to_string(),
            _ => val.to_string(),
        };
    } else if line.starts_with("group ") {
        c.group = line.strip_prefix("group ").unwrap_or("").trim().to_string();
    } else if line.starts_with("allowed_in ") {
        c.allowed_in = line.strip_prefix("allowed_in ").unwrap_or("").trim().to_string();
    }
}

fn extract_quoted(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
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
        Commands::Lex { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            for token in &tokens {
                println!("{:?}", token);
            }
        }
        Commands::Parse { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            match veil_parser::parse(&tokens) {
                Ok(sol) => {
                    println!("{}", serde_json::to_string_pretty(&sol).unwrap());
                }
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::Check { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            match veil_parser::parse(&tokens) {
                Ok(sol) => {
                    let graph = veil_ir::build_ir(&sol);
                    println!("✓ Parsed: {}", sol.name);
                    println!("  Nodes: {}", graph.nodes.len());
                    println!("  Edges: {}", graph.edges.len());
                    // Output IR JSON to stdout for viewer consumption
                    println!("\n{}", serde_json::to_string_pretty(&graph).unwrap());
                }
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            }
        }
        Commands::Gen { file, output } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            let sol = match veil_parser::parse(&tokens) {
                Ok(sol) => sol,
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            };

            let project = veil_codegen::generate(&sol);
            for file in &project.files {
                let path = output.join(&file.path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("Failed to create directory");
                }
                std::fs::write(&path, &file.content).expect("Failed to write file");
            }
            println!(
                "✓ Generated {} files in {}",
                project.files.len(),
                output.display()
            );
        }
        Commands::Emit { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            let sol = match veil_parser::parse(&tokens) {
                Ok(sol) => sol,
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            };
            let output = veil_ir::serialize_solution(&sol);
            print!("{}", output);
        }
        Commands::Serve { file, port } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);

            // Use parse_file to detect type
            let veil_file = match veil_parser::parse_file(&tokens) {
                Ok(f) => f,
                Err(errors) => {
                    eprintln!("Parse errors:");
                    for err in &errors {
                        eprintln!("  {}", err);
                    }
                    std::process::exit(1);
                }
            };

            // Build IR based on file type
            let graph = match &veil_file {
                veil_ir::VeilFile::Solution(sol) => veil_ir::build_ir(sol),
                veil_ir::VeilFile::Package(pkg) => {
                    // For packages, build IR from internal items
                    let sol = veil_ir::Solution {
                        name: pkg.name.clone(),
                        span: pkg.span,
                        items: pkg.items.clone(),
                    };
                    veil_ir::build_ir(&sol)
                }
                veil_ir::VeilFile::Composition(comp) => {
                    // For compositions, resolve packages and build filtered IR
                    let search_dir = file.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
                    let search_paths = vec![search_dir];

                    let mut resolved = Vec::new();
                    let found = veil_ir::find_package_files(&comp.imports, &search_paths);
                    for result in found {
                        match result {
                            Ok((imp, path)) => {
                                let pkg_source = std::fs::read_to_string(&path)
                                    .expect("Failed to read package");
                                let pkg_tokens = veil_parser::lex(&pkg_source);
                                if let Ok(veil_ir::VeilFile::Package(pkg)) =
                                    veil_parser::parse_file(&pkg_tokens)
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

            let graph_json = serde_json::to_string(&graph).unwrap();
            let node_count = graph.nodes.len();
            let edge_count = graph.edges.len();

            // Also serve the VEIL source for the code panel
            let veil_source = source.clone();

            println!("✓ Serving VEIL IR ({} nodes, {} edges)", node_count, edge_count);
            println!("  API: http://localhost:{}/api/ir", port);
            println!("  Source: http://localhost:{}/api/source", port);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                // Build palette config from loaded layers
                let palette_json = serde_json::to_string(&build_palette_config(&file)).unwrap();

                let app = axum::Router::new()
                    .route("/api/ir", axum::routing::get({
                        let json = graph_json.clone();
                        move || async move {
                            (
                                [(axum::http::header::CONTENT_TYPE, "application/json")],
                                json.clone(),
                            )
                        }
                    }))
                    .route("/api/source", axum::routing::get({
                        let src = veil_source.clone();
                        move || async move {
                            (
                                [(axum::http::header::CONTENT_TYPE, "text/plain")],
                                src.clone(),
                            )
                        }
                    }))
                    .route("/api/palette", axum::routing::get({
                        let palette = palette_json.clone();
                        move || async move {
                            (
                                [(axum::http::header::CONTENT_TYPE, "application/json")],
                                palette.clone(),
                            )
                        }
                    }))
                    .layer(
                        tower_http::cors::CorsLayer::new()
                            .allow_origin(tower_http::cors::Any)
                            .allow_methods(tower_http::cors::Any)
                            .allow_headers(tower_http::cors::Any),
                    );

                let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
                    .await
                    .unwrap();
                println!("  Listening on port {}", port);
                axum::serve(listener, app).await.unwrap();
            });
        }
    }
}
