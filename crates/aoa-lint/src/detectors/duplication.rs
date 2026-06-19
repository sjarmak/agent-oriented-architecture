use std::collections::BTreeSet;

use crate::category::SmellCategory;
use crate::detectors::LintedFile;
use crate::finding::Finding;

/// Flag headings that appear more than once in the same file. A repeated
/// heading is redundant structure (catalog: duplication).
pub fn detect(file: &LintedFile) -> Vec<Finding> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    let mut findings = Vec::new();

    for line in file.text.lines() {
        let Some(title) = heading_title(line) else {
            continue;
        };
        if !seen.insert(title.clone()) && reported.insert(title.clone()) {
            findings.push(Finding {
                file: file.path.clone(),
                message: format!("duplicate heading: '{title}'"),
                category: SmellCategory::Duplication,
            });
        }
    }
    findings
}

/// The normalized title of a markdown ATX heading line, if `line` is one.
fn heading_title(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let title = trimmed.trim_start_matches('#').trim();
    if title.is_empty() {
        return None;
    }
    Some(title.to_lowercase())
}
