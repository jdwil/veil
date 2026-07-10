//! Format-preserving structured edits for `.layer` files (DSL-006 / DSL-008).

use veil_ir::EditOp;

/// Apply package-style EditOps to layer source text where possible.
pub fn apply_layer_edits(source: &str, edits: &[EditOp]) -> Result<String, String> {
    let mut src = source.to_string();
    for op in edits {
        src = match op {
            EditOp::Rename { name, .. } => rename_first_construct(&src, name)?,
            EditOp::CreateConstruct {
                keyword, name, ..
            } => append_construct(&src, keyword, name)?,
            EditOp::DeleteConstruct { .. } => {
                return Err(
                    "DeleteConstruct for layers requires construct name; use delete_layer_construct helper or source edit"
                        .into(),
                );
            }
            EditOp::SetAnnotations { .. }
            | EditOp::SetFields { .. }
            | EditOp::SetMethods { .. }
            | EditOp::SetBody { .. } => {
                return Err(format!(
                    "edit op {:?} is package-shaped; for layers use source write or create/rename",
                    std::mem::discriminant(op)
                ));
            }
        };
    }
    // Validate
    let name = src
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("pkg ")
                .map(|r| r.split_whitespace().next().unwrap_or("layer").to_string())
        })
        .unwrap_or_else(|| "layer".into());
    veil_ir::parse_layer_file(&src, &name).map_err(|e| format!("layer invalid after edit: {e}"))?;
    Ok(src)
}

/// Rename the first `construct X` name and matching `kw` if equal to old name.
fn rename_first_construct(source: &str, new_name: &str) -> Result<String, String> {
    // Used when span points at a construct — for layers we rename by scanning selection via properties.
    // Fallback: rename first construct block's name token after `construct `.
    let mut out = String::new();
    let mut done = false;
    for line in source.lines() {
        let t = line.trim_start();
        if !done {
            if let Some(rest) = t.strip_prefix("construct ") {
                let old = rest.split_whitespace().next().unwrap_or("");
                if !old.is_empty() {
                    let replaced = line.replacen(old, new_name, 1);
                    out.push_str(&replaced);
                    out.push('\n');
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !done {
        return Err("no construct to rename".into());
    }
    Ok(out)
}

/// Delete a construct block by name (format-preserving).
pub fn delete_layer_construct(source: &str, construct_name: &str) -> Result<String, String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut found = false;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.starts_with(&format!("construct {construct_name}"))
            || t == format!("construct {construct_name}")
        {
            found = true;
            let base_indent = lines[i].len() - lines[i].trim_start().len();
            i += 1;
            while i < lines.len() {
                let line = lines[i];
                if line.trim().is_empty() {
                    i += 1;
                    continue;
                }
                let ind = line.len() - line.trim_start().len();
                if ind <= base_indent && !line.trim().is_empty() {
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }
    if !found {
        return Err(format!("construct '{construct_name}' not found"));
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}

fn append_construct(source: &str, keyword: &str, name: &str) -> Result<String, String> {
    let kw = if keyword.is_empty() {
        name.to_lowercase()
    } else {
        keyword.to_string()
    };
    let block = format!(
        "\n  construct {name}\n    kw {kw}\n    mt struct\n    desc \"{name}\"\n    visual\n      icon \"📦\"\n      color \"#6366f1\"\n      label \"{name}\"\n    group domain\n"
    );
    let mut s = source.to_string();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&block);
    Ok(s)
}

/// Set a simple key line under a construct (e.g. `kw`, `desc`) — first match.
pub fn set_construct_field(
    source: &str,
    construct_name: &str,
    field: &str,
    value: &str,
) -> Result<String, String> {
    let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_target = false;
    let mut base_indent = 0usize;
    let mut found = false;
    while i < lines.len() {
        let line = &lines[i];
        let t = line.trim_start();
        let ind = line.len() - line.trim_start().len();
        if t.starts_with("construct ") {
            let name = t.strip_prefix("construct ").unwrap_or("").split_whitespace().next().unwrap_or("");
            in_target = name == construct_name;
            if in_target {
                base_indent = ind;
                found = true;
            }
            out.push(line.clone());
            i += 1;
            continue;
        }
        if in_target {
            if !t.is_empty() && ind <= base_indent {
                in_target = false;
            } else if t.starts_with(field) && (t[field.len()..].starts_with(' ') || t == field) {
                let prefix = &line[..line.len() - t.len()];
                if field == "desc" {
                    out.push(format!("{prefix}{field} \"{value}\""));
                } else {
                    out.push(format!("{prefix}{field} {value}"));
                }
                i += 1;
                // skip until end of construct after update? only replace one field
                while i < lines.len() {
                    out.push(lines[i].clone());
                    i += 1;
                }
                return Ok(out.join("\n") + "\n");
            }
        }
        out.push(line.clone());
        i += 1;
    }
    if !found {
        return Err(format!("construct '{construct_name}' not found"));
    }
    Err(format!("field '{field}' not found on construct '{construct_name}'"))
}
