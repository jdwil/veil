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

/// Build the palette configuration from loaded layers.
/// For now this is hardcoded from the DDD layer — will be dynamic later.
fn build_palette_config() -> Vec<PaletteConstruct> {
    vec![
        // Solution level
        PaletteConstruct { name: "Context".into(), kind: "Module".into(), icon: "📦".into(), color: "#8b5cf6".into(), label: "Bounded Context".into(), group: "".into(), allowed_in: "top".into() },
        PaletteConstruct { name: "Saga".into(), kind: "Saga".into(), icon: "🔄".into(), color: "#dc2626".into(), label: "Saga".into(), group: "".into(), allowed_in: "top".into() },
        // Domain group
        PaletteConstruct { name: "Aggregate".into(), kind: "TypeDef".into(), icon: "🧩".into(), color: "#ec4899".into(), label: "Aggregate".into(), group: "domain".into(), allowed_in: "Module".into() },
        PaletteConstruct { name: "Entity".into(), kind: "TypeDef".into(), icon: "🔑".into(), color: "#f43f5e".into(), label: "Entity".into(), group: "domain".into(), allowed_in: "Module".into() },
        PaletteConstruct { name: "ValueObject".into(), kind: "TypeDef".into(), icon: "💎".into(), color: "#14b8a6".into(), label: "Value Object".into(), group: "domain".into(), allowed_in: "Module".into() },
        PaletteConstruct { name: "Port".into(), kind: "Interface".into(), icon: "🔌".into(), color: "#10b981".into(), label: "Port".into(), group: "domain".into(), allowed_in: "Module".into() },
        PaletteConstruct { name: "DomainService".into(), kind: "Flow".into(), icon: "🖥️".into(), color: "#0ea5e9".into(), label: "Domain Service".into(), group: "domain".into(), allowed_in: "Module".into() },
        // Infrastructure group
        PaletteConstruct { name: "Adapter".into(), kind: "Implementation".into(), icon: "🔗".into(), color: "#a855f7".into(), label: "Adapter".into(), group: "infrastructure".into(), allowed_in: "Module".into() },
        // Aggregate children
        PaletteConstruct { name: "Event".into(), kind: "TypeDef".into(), icon: "⚡".into(), color: "#f59e0b".into(), label: "Domain Event".into(), group: "".into(), allowed_in: "TypeDef".into() },
        PaletteConstruct { name: "Command".into(), kind: "TypeDef".into(), icon: "📨".into(), color: "#3b82f6".into(), label: "Command".into(), group: "".into(), allowed_in: "TypeDef".into() },
        // Flow children
        PaletteConstruct { name: "Step".into(), kind: "Step".into(), icon: "▶️".into(), color: "#64748b".into(), label: "Step".into(), group: "".into(), allowed_in: "Flow".into() },
    ]
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
                let palette_json = serde_json::to_string(&build_palette_config()).unwrap();

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
