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
    if is_markdown(file) {
        findings.extend(duplication::detect(file));
        findings.extend(verbosity::detect(file));
    }
    findings.extend(stale_reference::detect(file));
    findings.extend(overbroad_glob::detect(file));
    findings.extend(contradiction::detect(file));
    findings
}

const MARKDOWN_EXTENSIONS: [&str; 4] = ["md", "markdown", "mdc", "mdx"];

fn is_markdown(file: &LintedFile) -> bool {
    has_markdown_name(&file.path) && !file.text.starts_with("#!")
}

fn has_markdown_name(path: &Path) -> bool {
    match path.extension() {
        None => true,
        Some(extension) => MARKDOWN_EXTENSIONS
            .iter()
            .any(|markdown| extension.eq_ignore_ascii_case(markdown)),
    }
}

#[cfg(test)]
mod tests {
    use super::{has_markdown_name, is_markdown, LintedFile};
    use std::path::{Path, PathBuf};

    #[test]
    fn markdown_extensions_match_in_any_case() {
        for path in [
            "AGENTS.md",
            "docs/GUIDE.MD",
            "rules/style.mdc",
            "a.markdown",
            "b.mdx",
        ] {
            assert!(has_markdown_name(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn extensionless_context_files_are_markdown() {
        for path in [".cursorrules", "AGENTS"] {
            assert!(has_markdown_name(Path::new(path)), "{path}");
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
            assert!(!has_markdown_name(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn a_shebang_marks_a_script_whatever_its_name() {
        for path in ["bin/deploy", "notes.md"] {
            let script = LintedFile {
                path: PathBuf::from(path),
                text: "#!/usr/bin/env bash\necho hi\n".to_string(),
            };
            assert!(!is_markdown(&script), "{path}");
        }
    }

    #[test]
    fn a_heading_on_the_first_line_is_not_a_shebang() {
        let context = LintedFile {
            path: PathBuf::from(".cursorrules"),
            text: "# Style\n".to_string(),
        };
        assert!(is_markdown(&context));
    }
}
