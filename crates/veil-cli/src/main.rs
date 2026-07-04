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
        Commands::Check { file } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let (sol, registry) = parse_solution_or_exit(&source, &file);
            let graph = veil_ir::build_ir(&sol);
            println!("✓ Parsed: {}", sol.name);
            println!("  Layers: {}", registry.layers.join(", "));
            println!("  Nodes: {}", graph.nodes.len());
            println!("  Edges: {}", graph.edges.len());

            let errors = veil_ir::validate::validate_solution(&sol, &registry);
            if errors.is_empty() {
                println!("  Validation: ✓ all constraints pass");
            } else {
                println!("  Validation: ✗ {} error(s):", errors.len());
                for err in &errors {
                    println!("    ✗ {}", err);
                }
            }

            println!("\n{}", serde_json::to_string_pretty(&graph).unwrap());
        }
        Commands::Gen { file, output } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let (sol, _) = parse_solution_or_exit(&source, &file);

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
            let (sol, _) = parse_solution_or_exit(&source, &file);
            let output = veil_ir::serialize_solution(&sol);
            print!("{}", output);
        }
        Commands::Serve { file, port } => {
            let source = std::fs::read_to_string(&file).expect("Failed to read file");
            let tokens = veil_parser::lex(&source);
            let registry = registry_for(&file);

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

            let graph = match &veil_file {
                veil_ir::VeilFile::Solution(sol) => veil_ir::build_ir(sol),
                veil_ir::VeilFile::Package(pkg) => {
                    let sol = veil_ir::Solution {
                        name: pkg.name.clone(),
                        span: pkg.span,
                        uses: Vec::new(),
                        items: pkg.items.clone(),
                    };
                    veil_ir::build_ir(&sol)
                }
                veil_ir::VeilFile::Composition(comp) => {
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

            let graph_json = serde_json::to_string(&graph).unwrap();
            let node_count = graph.nodes.len();
            let edge_count = graph.edges.len();
            let veil_source = source.clone();

            println!("✓ Serving VEIL IR ({} nodes, {} edges)", node_count, edge_count);
            println!("  Layers: {}", registry.layers.join(", "));
            println!("  API: http://localhost:{}/api/ir", port);
            println!("  Source: http://localhost:{}/api/source", port);
            println!("  Palette: http://localhost:{}/api/palette", port);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                // Palette straight from the registry — visuals, groups,
                // placement rules, and statements, all layer-defined.
                let palette_json =
                    serde_json::to_string(&veil_ir::palette_from_registry(&registry)).unwrap();

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
