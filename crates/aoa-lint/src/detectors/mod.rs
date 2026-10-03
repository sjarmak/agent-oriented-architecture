use std::path::{Path, PathBuf};

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
    if is_markdown(&file.path) {
        findings.extend(duplication::detect(file));
        findings.extend(verbosity::detect(file));
    }
    findings.extend(stale_reference::detect(file));
    findings.extend(overbroad_glob::detect(file));
    findings.extend(contradiction::detect(file));
    findings
}

const MARKDOWN_EXTENSIONS: [&str; 4] = ["md", "markdown", "mdc", "mdx"];

fn is_markdown(path: &Path) -> bool {
    match path.extension() {
        None => true,
        Some(extension) => MARKDOWN_EXTENSIONS
            .iter()
            .any(|markdown| extension.eq_ignore_ascii_case(markdown)),
    }
}

#[cfg(test)]
mod tests {
    use super::is_markdown;
    use std::path::Path;

    #[test]
    fn markdown_extensions_match_in_any_case() {
        for path in [
            "AGENTS.md",
            "docs/GUIDE.MD",
            "rules/style.mdc",
            "a.markdown",
            "b.mdx",
        ] {
            assert!(is_markdown(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn extensionless_context_files_are_markdown() {
        for path in [".cursorrules", "AGENTS"] {
            assert!(is_markdown(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn other_extensions_are_not_markdown() {
        for path in [
            "snapshot.toml",
            "scripts/sync.ts",
            "notes.md.bak",
            "Cargo.lock",
        ] {
            assert!(!is_markdown(Path::new(path)), "{path}");
        }
    }
}
