#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use aoa_trace::{resolve_repository_root, RepositoryRootError};

fn init_git_repo(path: &Path) {
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .status()
        .expect("git is available for repository-boundary tests");
    assert!(initialized.success(), "git init failed for test fixture");
}

fn assert_refused_at(resolved: Result<std::path::PathBuf, RepositoryRootError>, refused: &Path) {
    match resolved {
        Err(RepositoryRootError::NotAGitRoot { marker, reason }) => {
            assert_eq!(marker, refused.join(".git"));
            assert!(reason.contains("dubious ownership"), "{reason}");
            assert!(reason.contains("--show-toplevel"), "{reason}");
        }
        other => panic!(
            "expected git's refusal of {}, got {other:?}",
            refused.display()
        ),
    }
}

#[test]
fn reports_a_refused_nested_repository_instead_of_its_parent() {
    let fixture = tempfile::tempdir().unwrap();
    let parent = fixture.path().canonicalize().unwrap().join("parent");
    let nested = parent.join("nested");
    let below_nested = nested.join("src");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::create_dir(&below_nested).unwrap();
    let only_parent_is_safe = fixture.path().join("only-parent-is-safe.gitconfig");
    std::fs::write(
        &only_parent_is_safe,
        format!("[safe]\n\tdirectory = {}\n", parent.display()),
    )
    .unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", &only_parent_is_safe);
    std::env::set_var("GIT_CONFIG_SYSTEM", "/dev/null");
    std::env::set_var("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1");

    assert_eq!(
        resolve_repository_root(&parent).unwrap(),
        parent,
        "the fixture must leave the parent repository acceptable to git"
    );
    assert_refused_at(resolve_repository_root(&nested), &nested);
    assert_refused_at(resolve_repository_root(&below_nested), &nested);
}
