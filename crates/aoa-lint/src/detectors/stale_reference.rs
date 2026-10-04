use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;
use aoa_budget::{markdown_link_paths, normalize_path};

/// Flag markdown links whose local target does not exist on disk (a dead link).
/// External links (`http`, `https`, `mailto`) and pure anchors are ignored.
/// Catalog: stale reference.
pub fn detect(file: &LintedFile, absent: &BTreeSet<PathBuf>) -> Vec<Finding> {
    let base_dir = file.path.parent().unwrap_or(Path::new("."));
    markdown_link_paths(&file.text)
        .into_iter()
        .filter(|local| absent.contains(&normalize_path(&base_dir.join(local))))
        .map(|local| Finding {
            file: file.path.clone(),
            message: format!("stale reference: linked file '{local}' does not exist"),
            category: SmellCategory::StaleReference,
        })
        .collect()
}
