//! Fold chat-pane attachments into the agent prompt and ACP vision blocks.
//!
//! The runtime AgentDock reads dropped files in the browser (text inlined,
//! images/pdf as base64). This module:
//! - persists bytes under `$TMP/veil-chat-attachments/{turn}/`
//! - avoids double-inlining when the client already added `# Attached documents`
//! - returns ACP image parts so diagrams are visible on the same turn

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const ATTACHED_DOCS_HEADING: &str = "# Attached documents";

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChatAttachment {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub data_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentImage {
    pub mime_type: String,
    pub data_base64: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AppliedAttachments {
    pub prompt: String,
    pub images: Vec<AgentImage>,
    pub saved_paths: Vec<String>,
}

pub fn apply_attachments(prompt: &str, attachments: &[ChatAttachment]) -> AppliedAttachments {
    if attachments.is_empty() {
        return AppliedAttachments {
            prompt: prompt.to_string(),
            ..Default::default()
        };
    }

    let saved = persist_attachments(attachments);
    let images = attachments
        .iter()
        .filter(|a| is_image(a) && a.data_base64.as_ref().is_some_and(|s| !s.is_empty()))
        .map(|a| AgentImage {
            mime_type: a
                .mime_type
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "image/png".into()),
            data_base64: a.data_base64.clone().unwrap_or_default(),
            name: Some(a.name.clone()),
        })
        .collect();

    let already = prompt.contains(ATTACHED_DOCS_HEADING);
    let mut prompt = prompt.to_string();
    if !already {
        prompt = fold_text_into_prompt(&prompt, attachments, &saved);
    } else if !saved.is_empty() {
        prompt.push_str("\n\n# Attachment files on disk\n");
        for p in &saved {
            prompt.push_str(&format!("- `{p}`\n"));
        }
        prompt.push_str(
            "Read those paths if you need the original bytes (PDF, Office, large binaries).\n",
        );
    }

    AppliedAttachments {
        prompt,
        images,
        saved_paths: saved,
    }
}

fn is_image(a: &ChatAttachment) -> bool {
    if a.kind.as_deref() == Some("image") {
        return true;
    }
    a.mime_type
        .as_deref()
        .map(|m| m.starts_with("image/") && !m.contains("svg"))
        .unwrap_or(false)
}

fn fold_text_into_prompt(
    prompt: &str,
    attachments: &[ChatAttachment],
    saved: &[String],
) -> String {
    let mut out = prompt.trim().to_string();
    if out.is_empty() {
        out.push_str(
            "Please read the attached documents and use them as the source of truth for this request.",
        );
    }
    out.push_str("\n\n");
    out.push_str(ATTACHED_DOCS_HEADING);
    out.push_str(
        "\nThe operator dropped files into the agent chat. Read every attachment before answering.\n",
    );
    for a in attachments {
        let mime = a.mime_type.as_deref().unwrap_or("application/octet-stream");
        out.push_str(&format!("\n## {} ({mime})\n", a.name));
        if let Some(text) = a.text.as_ref().filter(|t| !t.is_empty()) {
            out.push_str("```\n");
            out.push_str(text);
            out.push_str("\n```\n");
        } else if is_image(a) {
            out.push_str("[Raster image — also sent as a vision content block on this turn.]\n");
        } else {
            out.push_str("[Binary attached");
            if let Some(p) = saved.iter().find(|p| p.ends_with(&a.name)) {
                out.push_str(&format!("; on disk at `{p}`"));
            }
            out.push_str(".]\n");
        }
    }
    out
}

fn persist_attachments(attachments: &[ChatAttachment]) -> Vec<String> {
    let mut saved = Vec::new();
    let dir = attachment_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, "chat attachments: mkdir failed");
        return saved;
    }
    for (i, a) in attachments.iter().enumerate() {
        let Some(b64) = a.data_base64.as_ref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let bytes = match decode_b64(b64) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(file = %a.name, error = %e, "chat attachments: base64 decode failed");
                continue;
            }
        };
        let safe = sanitize_filename(&a.name, i);
        let path = dir.join(&safe);
        if let Err(e) = std::fs::write(&path, &bytes) {
            tracing::warn!(file = %a.name, error = %e, "chat attachments: write failed");
            continue;
        }
        saved.push(path.to_string_lossy().to_string());
    }
    saved
}

fn attachment_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir()
        .join("veil-chat-attachments")
        .join(format!("t{stamp}"))
}

fn sanitize_filename(name: &str, idx: usize) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        format!("file-{idx}")
    } else {
        cleaned
    }
}

fn decode_b64(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    // Prefer the `base64` crate when present; fall back to a small decoder
    // so this module stays usable in unit tests without extra wiring.
    decode_b64_std(s)
}

fn decode_b64_std(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 {
        return Err("invalid base64 length".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks_exact(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let a = val(chunk[0]).ok_or("invalid base64")?;
        let b = val(chunk[1]).ok_or("invalid base64")?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            val(chunk[2]).ok_or("invalid base64")?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            val(chunk[3]).ok_or("invalid base64")?
        };
        let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | (d as u32);
        out.push(((n >> 16) & 0xff) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if pad < 1 {
            out.push((n & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_passthrough() {
        let applied = apply_attachments("hello", &[]);
        assert_eq!(applied.prompt, "hello");
        assert!(applied.images.is_empty());
    }

    #[test]
    fn inlines_text_when_client_did_not() {
        let applied = apply_attachments(
            "use this diagram set",
            &[ChatAttachment {
                name: "layers.md".into(),
                mime_type: Some("text/markdown".into()),
                kind: Some("text".into()),
                text: Some("# Layer A\nuses B".into()),
                data_base64: None,
            }],
        );
        assert!(applied.prompt.contains(ATTACHED_DOCS_HEADING));
        assert!(applied.prompt.contains("Layer A"));
        assert!(applied.prompt.contains("use this diagram set"));
        assert!(applied.images.is_empty());
    }

    #[test]
    fn does_not_double_inline_when_heading_present() {
        let prompt = format!(
            "look\n\n{ATTACHED_DOCS_HEADING}\n## layers.md\n```\n# Layer A\n```\n"
        );
        let applied = apply_attachments(
            &prompt,
            &[ChatAttachment {
                name: "shot.png".into(),
                mime_type: Some("image/png".into()),
                kind: Some("image".into()),
                text: None,
                data_base64: Some("aGVsbG8=".into()),
            }],
        );
        assert_eq!(
            applied.prompt.matches(ATTACHED_DOCS_HEADING).count(),
            1,
            "must not add a second attached-docs section"
        );
        assert_eq!(applied.images.len(), 1);
        assert_eq!(applied.images[0].mime_type, "image/png");
        assert!(!applied.saved_paths.is_empty());
    }

    #[test]
    fn decode_png_header_b64() {
        // "hello" base64
        let got = decode_b64_std("aGVsbG8=").unwrap();
        assert_eq!(got, b"hello");
    }

    #[test]
    fn sanitize_strips_path() {
        assert_eq!(sanitize_filename("../../evil name.png", 0), "evil_name.png");
        assert_eq!(sanitize_filename("", 3), "file-3");
    }

    #[test]
    fn deserializes_client_camel_case() {
        let a: ChatAttachment = serde_json::from_str(
            r#"{"name":"erd.png","mimeType":"image/png","kind":"image","dataBase64":"aGVsbG8="}"#,
        )
        .unwrap();
        assert_eq!(a.name, "erd.png");
        assert_eq!(a.mime_type.as_deref(), Some("image/png"));
        assert_eq!(a.data_base64.as_deref(), Some("aGVsbG8="));
        assert!(is_image(&a));
    }
}
