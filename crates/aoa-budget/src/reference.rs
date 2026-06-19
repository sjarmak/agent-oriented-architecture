use std::path::{Path, PathBuf};

/// A reference to another context file, already resolved against the directory
/// of the file that contained it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub target: PathBuf,
}

/// Extract local file references from the text of a context file.
///
/// Two reference syntaxes are supported:
/// - Markdown links `[label](path)` — the path inside the parentheses.
/// - `@path` includes (CLAUDE.md / AGENTS.md style) — a token beginning with
///   `@` followed by a relative path.
///
/// External targets (`http://`, `https://`, `mailto:`) and pure anchors
/// (`#section`) are skipped: they are not local files. Every returned path is
/// resolved relative to `base_dir` (the directory of the referencing file).
pub fn extract_references(text: &str, base_dir: &Path) -> Vec<Reference> {
    let mut refs = Vec::new();
    for raw in markdown_link_targets(text)
        .into_iter()
        .chain(at_include_targets(text))
    {
        if let Some(local) = local_path(&raw) {
            refs.push(Reference {
                target: base_dir.join(local),
            });
        }
    }
    refs
}

fn local_path(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:")
    {
        return None;
    }
    Some(trimmed.split('#').next().unwrap_or(trimmed))
}

fn markdown_link_targets(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            if let Some(end) = text[i + 2..].find(')') {
                out.push(text[i + 2..i + 2 + end].to_string());
                i = i + 2 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn at_include_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split(|c: char| c.is_whitespace()) {
        if let Some(rest) = token.strip_prefix('@') {
            if !rest.is_empty() {
                out.push(rest.trim_end_matches(['.', ',', ')']).to_string());
            }
        }
    }
    out
}
