use std::fs;
use std::path::{Path, PathBuf};

use aoa_lint::{discover_context_roots, lint_context_roots, LintError, SmellCategory};
use tempfile::TempDir;

const DUPLICATED_HEADING: &str = "# Rules\n\nfirst\n\n# Rules\n\nsecond\n";

fn write(base: &Path, relative: &str, text: &str) -> PathBuf {
    let path = base.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(&path, text).expect("write file");
    path
}

fn relative_roots(base: &Path) -> Vec<PathBuf> {
    discover_context_roots(base)
        .expect("discovery succeeds")
        .into_iter()
        .map(|root| root.strip_prefix(base).expect("under base").to_path_buf())
        .collect()
}

#[test]
fn discovers_every_nested_agents_and_claude_file_in_path_order() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "CLAUDE.md", "# Root\n");
    write(dir.path(), "services/api/CLAUDE.md", "# Api\n");
    write(dir.path(), "services/web/AGENTS.md", "# Web\n");
    write(dir.path(), ".claude/CLAUDE.md", "# Hidden\n");
    write(dir.path(), "services/web/README.md", "# Not a root\n");

    assert_eq!(
        relative_roots(dir.path()),
        [
            PathBuf::from(".claude/CLAUDE.md"),
            PathBuf::from("CLAUDE.md"),
            PathBuf::from("services/api/CLAUDE.md"),
            PathBuf::from("services/web/AGENTS.md"),
        ]
    );
}

#[test]
fn discovery_skips_git_metadata_and_gitignored_directories() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), ".gitignore", "vendor/\n");
    write(dir.path(), "AGENTS.md", "# Root\n");
    write(dir.path(), "vendor/pkg/CLAUDE.md", "# Vendored\n");
    write(dir.path(), ".git/CLAUDE.md", "# Metadata\n");

    assert_eq!(relative_roots(dir.path()), [PathBuf::from("AGENTS.md")]);
}

#[test]
fn discovery_of_a_missing_directory_fails() {
    let dir = TempDir::new().expect("tempdir");
    let result = discover_context_roots(&dir.path().join("absent"));
    assert!(matches!(result, Err(LintError::Walk { .. })));
}

#[test]
fn every_root_is_linted_and_a_shared_file_is_linted_once() {
    let dir = TempDir::new().expect("tempdir");
    let root = write(
        dir.path(),
        "CLAUDE.md",
        "# Root\n\nSee [shared](shared.md).\n",
    );
    let nested = write(
        dir.path(),
        "pkg/CLAUDE.md",
        &format!("{DUPLICATED_HEADING}\nSee [shared](../shared.md).\n"),
    );
    write(dir.path(), "shared.md", DUPLICATED_HEADING);

    let report = lint_context_roots(&[root, nested], "o200k_base").expect("lint succeeds");

    let duplicated: Vec<PathBuf> = report
        .findings
        .iter()
        .filter(|finding| finding.category == SmellCategory::Duplication)
        .map(|finding| {
            finding
                .file
                .strip_prefix(dir.path())
                .expect("under base")
                .to_path_buf()
        })
        .collect();
    assert_eq!(
        duplicated,
        [PathBuf::from("shared.md"), PathBuf::from("pkg/CLAUDE.md")]
    );
    assert_eq!(report.budget.files.len(), 3);
}

#[test]
fn linting_no_roots_fails() {
    let result = lint_context_roots(&[], "o200k_base");
    assert!(matches!(result, Err(LintError::NoRoots)));
}
