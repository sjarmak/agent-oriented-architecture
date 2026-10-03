use std::path::Path;

use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;
use aoa_budget::{markdown_link_paths, normalize_path};

/// Flag markdown links whose local target does not exist on disk (a dead link).
/// External links (`http`, `https`, `mailto`) and pure anchors are ignored.
/// Catalog: stale reference.
pub fn detect(file: &LintedFile) -> Vec<Finding> {
    let base_dir = file.path.parent().unwrap_or(Path::new("."));
    let mut findings = Vec::new();

    for local in markdown_link_paths(&file.text) {
        let target = normalize_path(&base_dir.join(&local));
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
