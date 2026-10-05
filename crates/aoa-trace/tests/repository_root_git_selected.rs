#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use aoa_trace::{resolve_repository_root, RepositoryRootError};

fn git(directory: &Path, args: &[&str]) {
    std::fs::create_dir_all(directory).unwrap();
    let ran = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("git is available for repository-boundary tests");
    assert!(
        ran.status.success(),
        "git {args:?} failed in {}: {}",
        directory.display(),
        String::from_utf8_lossy(&ran.stderr)
    );
}

fn git_toplevel_output(directory: &Path) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .unwrap()
}

fn assert_refused_naming(resolved: Result<PathBuf, RepositoryRootError>, candidate: &Path) {
    match resolved {
        Err(RepositoryRootError::NotAGitRoot { marker, reason }) => {
            assert_eq!(marker, candidate);
            assert!(reason.contains("--show-toplevel"), "{reason}");
        }
        other => panic!(
            "expected a refusal naming {}, got {other:?}",
            candidate.display()
        ),
    }
}

fn fixture_root() -> (tempfile::TempDir, PathBuf) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    (fixture, root)
}

#[test]
fn resolves_a_nested_repository_named_one_space_to_itself_not_its_parent() {
    let (_fixture, root) = fixture_root();
    let parent = root.join("parent");
    let nested = parent.join(" ");
    let below_nested = nested.join("src");
    git(&parent, &["init", "--quiet"]);
    git(&nested, &["init", "--quiet"]);
    std::fs::create_dir(&below_nested).unwrap();
    let reported = git_toplevel_output(&below_nested);
    assert!(reported.status.success());
    assert_eq!(
        reported.stdout,
        format!("{}\n", nested.display()).into_bytes(),
        "git must report the one-space directory followed by its line terminator"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);
    assert_eq!(resolve_repository_root(&below_nested).unwrap(), nested);
}

#[test]
fn refuses_a_nested_bare_repository_instead_of_resolving_to_its_parent() {
    let (_fixture, root) = fixture_root();
    let parent = root.join("parent");
    let bare = parent.join("nested.git");
    let below_bare = bare.join("objects");
    git(&parent, &["init", "--quiet"]);
    git(&bare, &["init", "--quiet", "--bare"]);
    assert!(
        !git_toplevel_output(&bare).status.success(),
        "git must select the bare repository, which has no worktree to report"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_naming(resolve_repository_root(&bare), &bare);
    assert_refused_naming(resolve_repository_root(&below_bare), &below_bare);
}

#[test]
fn refuses_a_path_inside_the_git_directory_of_the_root_itself() {
    let (_fixture, root) = fixture_root();
    let parent = root.join("parent");
    let git_dir = parent.join(".git");
    let below_git_dir = git_dir.join("objects");
    git(&parent, &["init", "--quiet"]);
    assert!(
        !git_toplevel_output(&git_dir).status.success(),
        "git reports no worktree from inside a git directory"
    );

    assert_refused_naming(resolve_repository_root(&git_dir), &git_dir);
    assert_refused_naming(resolve_repository_root(&below_git_dir), &below_git_dir);
}
