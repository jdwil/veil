//! TypeScript API client generation from VEIL expose blocks.
//!
//! Produces typed interfaces for inputs/outputs and async client classes
//! that call the API with correct types.

use veil_ir::ast::{ExposeBlock, Package};

use super::lower::{to_camel, type_to_ts};

/// Generated TypeScript project output.
pub struct TsProject {
    pub files: Vec<TsFile>,
}

pub struct TsFile {
    pub path: String,
    pub content: String,
}

// ─── Project Scaffolding ─────────────────────────────────────────────────────

pub fn gen_package_json(sol_name: &str) -> TsFile {
    let content = format!(
        r#"{{
  "name": "{}",
  "version": "0.1.0",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {{
    "build": "tsc",
    "dev": "tsc --watch"
  }},
  "devDependencies": {{
    "typescript": "^5.4.0"
  }}
}}
"#,
        sol_name
    );
    TsFile { path: "package.json".to_string(), content }
}

pub fn gen_tsconfig() -> TsFile {
    let content = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src",
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true
  },
  "include": ["src"]
}
"#.to_string();
    TsFile { path: "tsconfig.json".to_string(), content }
}

// ─── API Client Generation (from expose blocks) ──────────────────────────────

/// Generate a typed API client module from an expose block.
/// Produces typed interfaces for inputs/outputs and async functions that
/// call the API with correct types.
pub fn generate_api_client(pkg_name: &str, expose: &ExposeBlock) -> Vec<TsFile> {
    let mut files = Vec::new();
    let module_name = to_camel(pkg_name);

    let mut client = String::new();
    client.push_str("// Generated API client — typed bindings for the backend expose contract\n");
    client.push_str("// Do not edit — regenerated from the backend .veil package\n\n");

    // Generate input/output interfaces for each node
    for node in &expose.nodes {
        if !node.inputs.is_empty() {
            client.push_str(&format!("export interface {}Input {{\n", node.name));
            for field in &node.inputs {
                client.push_str(&format!("  {}: {};\n", to_camel(&field.name), type_to_ts(&field.type_expr)));
            }
            client.push_str("}\n\n");
        }

        if !node.outputs.is_empty() {
            client.push_str(&format!("export interface {}Output {{\n", node.name));
            for field in &node.outputs {
                client.push_str(&format!("  {}: {};\n", to_camel(&field.name), type_to_ts(&field.type_expr)));
            }
            client.push_str("}\n\n");
        }
    }

    // Generate the client class with typed methods
    client.push_str(&format!("export class {}Client {{\n", module_name));
    client.push_str("  private baseUrl: string;\n");
    client.push_str("  private headers: Record<string, string>;\n\n");
    client.push_str("  constructor(baseUrl: string, headers: Record<string, string> = {}) {\n");
    client.push_str("    this.baseUrl = baseUrl;\n");
    client.push_str("    this.headers = { 'Content-Type': 'application/json', ...headers };\n");
    client.push_str("  }\n\n");

    for node in &expose.nodes {
        let fn_name = to_camel(&node.name);
        let has_input = !node.inputs.is_empty();
        let has_output = !node.outputs.is_empty();

        let input_param = if has_input {
            format!("input: {}Input", node.name)
        } else {
            String::new()
        };
        let return_type = if has_output {
            format!("Promise<{}Output>", node.name)
        } else {
            "Promise<void>".to_string()
        };

        // Add description as JSDoc if available
        if let Some(desc) = &node.description {
            client.push_str(&format!("  /** {} */\n", desc));
        }

        client.push_str(&format!("  async {}({}): {} {{\n", fn_name, input_param, return_type));

        // Generate the endpoint path from the node name (kebab-case)
        let endpoint = node.name.chars().enumerate().map(|(i, c)| {
            if c.is_uppercase() && i > 0 { format!("-{}", c.to_lowercase()) }
            else { c.to_lowercase().to_string() }
        }).collect::<String>();

        if has_input {
            client.push_str(&format!(
                "    const res = await fetch(`${{this.baseUrl}}/{}`, {{\n      method: 'POST',\n      headers: this.headers,\n      body: JSON.stringify(input),\n    }});\n",
                endpoint
            ));
        } else {
            client.push_str(&format!(
                "    const res = await fetch(`${{this.baseUrl}}/{}`, {{\n      headers: this.headers,\n    }});\n",
                endpoint
            ));
        }

        client.push_str("    if (!res.ok) throw new Error(`API error: ${res.status}`);\n");
        if has_output {
            client.push_str("    return res.json();\n");
        }
        client.push_str("  }\n\n");
    }

    client.push_str("}\n");

    files.push(TsFile {
        path: format!("src/api/{}.ts", to_camel(pkg_name)),
        content: client,
    });

    files
}

/// Generate a typed API client from a Package's expose block.
/// Called when `veil gen package.veil -t ts` targets a pkg file.
pub fn generate_api_client_from_package(pkg: &Package) -> TsProject {
    let mut files = Vec::new();

    if let Some(expose) = &pkg.expose {
        files.extend(generate_api_client(&pkg.name, expose));
    }

    let mut index = String::from("// API client for ");
    index.push_str(&pkg.name);
    index.push_str(" — generated by VEIL\n\n");
    if pkg.expose.is_some() {
        index.push_str(&format!("export * from './api/{}';\n", to_camel(&pkg.name)));
    }
    files.push(TsFile { path: "src/index.ts".to_string(), content: index });

    files.push(gen_package_json(&to_camel(&pkg.name)));
    files.push(gen_tsconfig());

    TsProject { files }
}
