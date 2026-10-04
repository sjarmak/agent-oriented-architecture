use std::path::Path;

use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;
use aoa_budget::{markdown_link_paths, normalize_path};

/// Flag markdown links whose local target does not exist on disk (a dead link).
/// External links (`http`, `https`, `mailto`) and pure anchors are ignored.
/// Catalog: stale reference.
pub fn detect(file: &LintedFile, canonical_boundary: &Path) -> Vec<Finding> {
    let base_dir = file.path.parent().unwrap_or(Path::new("."));
    let mut findings = Vec::new();

    for local in markdown_link_paths(&file.text) {
        let target = normalize_path(&base_dir.join(&local));
        if leaves(&target, canonical_boundary) {
            continue;
        }
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

const MAX_LINK_HOPS: usize = 40;

fn leaves(target: &Path, canonical_boundary: &Path) -> bool {
    std::path::absolute(target).map_or(true, |target| {
        escapes(&normalize_path(&target), canonical_boundary, MAX_LINK_HOPS)
    })
}

fn escapes(target: &Path, canonical_boundary: &Path, hops: usize) -> bool {
    for ancestor in target.ancestors() {
        if let Ok(resolved) = ancestor.canonicalize() {
            return !resolved.starts_with(canonical_boundary);
        }
        if let Ok(named) = std::fs::read_link(ancestor) {
            if steps_back(&named) {
                return true;
            }
            let directory = ancestor
                .parent()
                .and_then(|parent| parent.canonicalize().ok());
            return match directory {
                Some(directory) if directory.starts_with(canonical_boundary) => {
                    hops > 0 && escapes(&directory.join(named), canonical_boundary, hops - 1)
                }
                _ => true,
            };
        }
    }
    true
}

fn steps_back(named: &Path) -> bool {
    named
        .components()
        .any(|component| component == std::path::Component::ParentDir)
}
