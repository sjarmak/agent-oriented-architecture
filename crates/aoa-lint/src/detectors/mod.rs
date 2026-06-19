use std::path::PathBuf;

use crate::finding::Finding;

mod contradiction;
mod duplication;
mod overbroad_glob;
mod stale_reference;
mod verbosity;

/// A single closure file presented to the detectors: its path and full text.
pub struct LintedFile {
    pub path: PathBuf,
    pub text: String,
}

/// Run every mechanical detector over `file`, returning all findings in a
/// stable order (detector order, then in-file order).
pub fn run_all(file: &LintedFile) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(duplication::detect(file));
    findings.extend(verbosity::detect(file));
    findings.extend(stale_reference::detect(file));
    findings.extend(overbroad_glob::detect(file));
    findings.extend(contradiction::detect(file));
    findings
}
