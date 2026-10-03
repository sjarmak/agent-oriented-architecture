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

pub fn markdown_link_paths(text: &str) -> Vec<String> {
    markdown_link_targets(text)
        .iter()
        .filter_map(|raw| local_path(raw))
        .map(str::to_string)
        .collect()
}

fn markdown_link_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        let Some((target, consumed)) = link_destination(after) else {
            break;
        };
        out.push(target.to_string());
        rest = &after[consumed..];
    }
    out
}

fn link_destination(after: &str) -> Option<(&str, usize)> {
    let unpadded = after.trim_start_matches([' ', '\t']);
    if let Some(bracketed) = unpadded.strip_prefix('<') {
        if let Some(close) = bracketed.find(['<', '>', '\n']) {
            if bracketed[close..].starts_with('>') {
                let padding = after.len() - unpadded.len();
                return Some((&bracketed[..close], padding + close + 2));
            }
        }
    }
    let end = after.find(')')?;
    Some((&after[..end], end + 1))
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
