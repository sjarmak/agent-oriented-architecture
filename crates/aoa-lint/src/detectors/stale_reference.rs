use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

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

fn leaves(target: &Path, canonical_boundary: &Path) -> bool {
    std::path::absolute(target).map_or(true, |target| {
        escapes(normalize_path(&target), canonical_boundary)
    })
}

enum Hop {
    Settled { escapes: bool },
    Follow(PathBuf),
}

fn escapes(target: PathBuf, canonical_boundary: &Path) -> bool {
    let mut followed = HashSet::new();
    let mut current = target;
    while followed.insert(current.clone()) {
        match next_hop(&current, canonical_boundary) {
            Hop::Settled { escapes } => return escapes,
            Hop::Follow(next) => current = next,
        }
    }
    false
}

fn next_hop(target: &Path, canonical_boundary: &Path) -> Hop {
    for ancestor in target.ancestors() {
        if let Ok(resolved) = ancestor.canonicalize() {
            return Hop::Settled {
                escapes: !resolved.starts_with(canonical_boundary),
            };
        }
        if let Ok(named) = std::fs::read_link(ancestor) {
            if steps_back_past_a_name(&named) {
                return Hop::Settled { escapes: true };
            }
            let directory = ancestor
                .parent()
                .and_then(|parent| parent.canonicalize().ok());
            return match directory {
                Some(directory) if directory.starts_with(canonical_boundary) => {
                    Hop::Follow(normalize_path(&directory.join(named)))
                }
                _ => Hop::Settled { escapes: true },
            };
        }
    }
    Hop::Settled { escapes: true }
}

fn steps_back_past_a_name(named: &Path) -> bool {
    named
        .components()
        .skip_while(|component| !matches!(component, Component::Normal(_)))
        .any(|component| component == Component::ParentDir)
}
