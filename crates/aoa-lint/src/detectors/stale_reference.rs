use std::path::{Path, PathBuf};

use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;

/// Flag markdown links whose local target does not exist on disk (a dead link).
/// External links (`http`, `https`, `mailto`) and pure anchors are ignored.
/// Catalog: stale reference.
pub fn detect(file: &LintedFile) -> Vec<Finding> {
    let base_dir = file.path.parent().unwrap_or(Path::new("."));
    let mut findings = Vec::new();

    for raw in markdown_link_targets(&file.text) {
        let Some(local) = local_path(&raw) else {
            continue;
        };
        let target = normalize(&base_dir.join(local));
        if !target.exists() {
            findings.push(Finding {
                file: file.path.clone(),
                message: format!("stale reference: linked file '{local}' does not exist"),
                category: SmellCategory::StaleReference,
            });
        }
    }
    findings
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

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        use std::path::Component::*;
        match component {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
