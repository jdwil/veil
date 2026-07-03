//! VEIL Validation — enforces layer constraints on parsed AST.
//!
//! Reads construct definitions from .layer files and validates that
//! .veil source files conform to the layer's rules.

use std::collections::HashMap;
use std::path::Path;

/// A construct definition from a .layer file.
#[derive(Debug, Clone)]
pub struct ConstructDef {
    pub name: String,
    /// What core primitive this maps to (mod, struct, trait, impl, fn)
    pub maps_to: String,
    /// What this construct is allowed to contain (child construct names)
    pub contains: Vec<String>,
    /// Where this construct can be placed
    pub allowed_in: String,
    /// Which group this construct belongs to
    pub group: String,
    /// Named constraints (e.g., "must_have_root", "sagas_only")
    pub constraints: Vec<String>,
}

/// The full schema from a layer — all construct definitions + rules.
#[derive(Debug, Clone)]
pub struct LayerSchema {
    pub name: String,
    pub constructs: HashMap<String, ConstructDef>,
}

/// A validation error with context.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
    pub construct: String,
    pub parent: String,
    pub hint: Option<String>,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] in {}: {}", self.construct, self.parent, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " (hint: {})", hint)?;
        }
        Ok(())
    }
}

/// Parse a .layer file into a LayerSchema.
pub fn parse_layer_schema(content: &str) -> LayerSchema {
    let mut schema = LayerSchema {
        name: String::new(),
        constructs: HashMap::new(),
    };

    let mut current_name: Option<String> = None;
    let mut current_def: Option<ConstructDef> = None;
    let mut in_contains = false;
    let mut in_constraints = false;

    for line in content.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        // Package name
        if trimmed.starts_with("pkg ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                schema.name = parts[1].to_string();
            }
            continue;
        }

        // New construct
        if trimmed.starts_with("construct ") {
            // Save previous
            if let (Some(name), Some(def)) = (current_name.take(), current_def.take()) {
                schema.constructs.insert(name, def);
            }
            let name = trimmed.strip_prefix("construct ").unwrap().trim().to_string();
            current_name = Some(name.clone());
            current_def = Some(ConstructDef {
                name,
                maps_to: String::new(),
                contains: Vec::new(),
                allowed_in: String::new(),
                group: String::new(),
                constraints: Vec::new(),
            });
            in_contains = false;
            in_constraints = false;
            continue;
        }

        // Skip statement blocks
        if trimmed.starts_with("statement ") {
            if let (Some(name), Some(def)) = (current_name.take(), current_def.take()) {
                schema.constructs.insert(name, def);
            }
            current_name = None;
            current_def = None;
            continue;
        }

        if let Some(ref mut def) = current_def {
            // Detect sub-blocks
            if trimmed == "contains" && indent <= 4 {
                in_contains = true;
                in_constraints = false;
                continue;
            }
            if trimmed == "constraints" && indent <= 4 {
                in_constraints = true;
                in_contains = false;
                continue;
            }
            if (trimmed == "visual" || trimmed.starts_with("maps_to ")
                || trimmed.starts_with("allowed_in ") || trimmed.starts_with("group ")
                || trimmed.starts_with("desc ")) && indent <= 4 {
                in_contains = false;
                in_constraints = false;
            }

            // Parse content based on current block
            if in_contains && indent > 4 && !trimmed.is_empty() {
                // Parse contains entries like "Saga[]", "group domain", "Event[]"
                let entry = trimmed.trim_end_matches("[]").to_string();
                if !entry.starts_with("group ") {
                    def.contains.push(entry);
                } else {
                    // "group domain" means this construct should contain a group named that
                    def.contains.push(trimmed.to_string());
                }
            } else if in_constraints && indent > 4 && !trimmed.is_empty() {
                def.constraints.push(trimmed.to_string());
            } else if indent <= 4 && !trimmed.is_empty() {
                // Top-level construct field
                if trimmed.starts_with("maps_to ") {
                    def.maps_to = trimmed.strip_prefix("maps_to ").unwrap().trim().to_string();
                } else if trimmed.starts_with("allowed_in ") {
                    def.allowed_in = trimmed.strip_prefix("allowed_in ").unwrap().trim().to_string();
                } else if trimmed.starts_with("group ") {
                    def.group = trimmed.strip_prefix("group ").unwrap().trim().to_string();
                }
            }
        }
    }

    // Save last construct
    if let (Some(name), Some(def)) = (current_name, current_def) {
        schema.constructs.insert(name, def);
    }

    schema
}

/// Validate a parsed solution against a layer schema.
/// Returns a list of validation errors.
pub fn validate_solution(
    items: &[(&str, &str, Vec<(&str, &str)>)], // (construct_type, name, children: [(type, name)])
    schema: &LayerSchema,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    for (construct_type, name, children) in items {
        // Check if this construct exists in the schema
        if let Some(def) = schema.constructs.get(*construct_type) {
            // Check children against 'contains' rules
            if !def.contains.is_empty() {
                for (child_type, child_name) in children {
                    let child_allowed = def.contains.iter().any(|c| {
                        let c_clean = c.trim_end_matches("[]");
                        c_clean == *child_type
                            || c.starts_with("group ")
                            || c_clean == "fn" && (*child_type == "DomainService" || *child_type == "Saga")
                    });

                    if !child_allowed && *child_type != "Group" {
                        errors.push(ValidationError {
                            message: format!(
                                "'{}' is not allowed directly inside '{}'",
                                child_type, construct_type
                            ),
                            construct: child_name.to_string(),
                            parent: name.to_string(),
                            hint: Some(format!(
                                "Allowed children: {}",
                                def.contains.join(", ")
                            )),
                        });
                    }
                }
            }

            // Check constraints
            for constraint in &def.constraints {
                match constraint.as_str() {
                    "sagas_only" => {
                        for (child_type, child_name) in children {
                            if *child_type != "Saga" && *child_type != "Group" {
                                errors.push(ValidationError {
                                    message: format!(
                                        "Orchestrator only allows Sagas, found '{}'",
                                        child_type
                                    ),
                                    construct: child_name.to_string(),
                                    parent: name.to_string(),
                                    hint: Some("Move non-saga constructs to a bounded context".to_string()),
                                });
                            }
                        }
                    }
                    "must_have_root" => {
                        let has_root = children.iter().any(|(t, _)| *t == "root");
                        if !has_root {
                            errors.push(ValidationError {
                                message: "Aggregate must define 'root' fields".to_string(),
                                construct: name.to_string(),
                                parent: name.to_string(),
                                hint: Some("Add a 'root' block with the aggregate's fields".to_string()),
                            });
                        }
                    }
                    "no_domain_constructs" => {
                        let domain_types = ["Aggregate", "Entity", "ValueObject", "Port", "Repository"];
                        for (child_type, child_name) in children {
                            if domain_types.contains(child_type) {
                                errors.push(ValidationError {
                                    message: format!(
                                        "Domain construct '{}' not allowed in Orchestrator",
                                        child_type
                                    ),
                                    construct: child_name.to_string(),
                                    parent: name.to_string(),
                                    hint: Some("Domain constructs belong in a bounded context".to_string()),
                                });
                            }
                        }
                    }
                    "requires_groups" => {
                        // All non-Group children are invalid — must be inside groups
                        for (child_type, child_name) in children {
                            if *child_type != "Group" {
                                errors.push(ValidationError {
                                    message: format!(
                                        "'{}' must be inside a group, not directly in '{}'",
                                        child_type, construct_type
                                    ),
                                    construct: child_name.to_string(),
                                    parent: name.to_string(),
                                    hint: Some("Wrap in 'group application' for sagas".to_string()),
                                });
                            }
                        }
                    }
                    "steps_have_compensation" => {
                        // TODO: check that saga steps have compensate blocks
                    }
                    _ => {}
                }
            }
        }
    }

    errors
}

/// Validate placement — is a construct allowed where it's placed?
pub fn validate_placement(
    construct_type: &str,
    parent_type: &str,
    parent_group: Option<&str>,
    schema: &LayerSchema,
) -> Option<ValidationError> {
    if let Some(def) = schema.constructs.get(construct_type) {
        if def.allowed_in.is_empty() {
            return None; // No placement restriction
        }

        let allowed = &def.allowed_in;

        // Check if placement matches
        let is_valid = match allowed.as_str() {
            "top" => parent_type == "Solution",
            "any" => true,
            _ => {
                // allowed_in specifies a NodeKind or parent construct name
                allowed == parent_type
                    // Also check if the group matches
                    || (parent_group.is_some() && def.group == parent_group.unwrap_or(""))
            }
        };

        if !is_valid {
            return Some(ValidationError {
                message: format!(
                    "'{}' is not allowed in '{}' (allowed_in: {})",
                    construct_type, parent_type, allowed
                ),
                construct: construct_type.to_string(),
                parent: parent_type.to_string(),
                hint: if def.group.is_empty() {
                    None
                } else {
                    Some(format!("Place inside a '{}' group", def.group))
                },
            });
        }
    }
    None
}

/// Load a layer schema from a file path.
pub fn load_layer_schema(path: &Path) -> Option<LayerSchema> {
    std::fs::read_to_string(path).ok().map(|content| parse_layer_schema(&content))
}

/// Load all layer schemas referenced by a .veil file.
pub fn load_referenced_schemas(veil_path: &Path) -> Vec<LayerSchema> {
    let dir = veil_path.parent().unwrap_or(Path::new("."));
    let content = std::fs::read_to_string(veil_path).unwrap_or_default();

    content.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("use "))
        .filter_map(|l| {
            let name = l.strip_prefix("use ")?.trim();
            let layer_path = dir.join(format!("{}.layer", name));
            load_layer_schema(&layer_path)
        })
        .collect()
}
