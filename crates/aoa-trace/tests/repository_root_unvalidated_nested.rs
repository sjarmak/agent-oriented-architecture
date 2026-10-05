use std::path::{Path, PathBuf};
use std::process::Command;

use aoa_trace::{resolve_repository_root, RepositoryRootError};

fn git(repo: &Path, args: &[&str]) {
    let ran = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=AOA Test",
            "-c",
            "user.email=aoa@example.invalid",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .expect("git is available for repository-boundary tests");
    assert!(
        ran.status.success(),
        "git {args:?} failed in {}: {}",
        repo.display(),
        String::from_utf8_lossy(&ran.stderr)
    );
}

fn init_git_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "--quiet"]);
}

fn repo_with_a_commit(path: &Path) {
    init_git_repo(path);
    git(
        path,
        &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
    );
}

fn git_toplevel(directory: &Path) -> PathBuf {
    let reported = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    assert!(reported.status.success(), "git must answer for the fixture");
    PathBuf::from(String::from_utf8(reported.stdout).unwrap().trim_end())
        .canonicalize()
        .unwrap()
}

fn assert_refused_at(resolved: Result<PathBuf, RepositoryRootError>, refused: &Path, why: &str) {
    match resolved {
        Err(RepositoryRootError::NotAGitRoot { marker, reason }) => {
            assert_eq!(marker, refused.join(".git"));
            assert!(reason.contains(why), "{reason}");
        }
        other => panic!(
            "expected a refusal naming {}, got {other:?}",
            refused.display()
        ),
    }
}

fn fixture_root() -> (tempfile::TempDir, PathBuf) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    (fixture, root)
}

#[test]
fn refuses_a_nested_submodule_instead_of_resolving_to_its_superproject() {
    let (_fixture, root) = fixture_root();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let below_nested = nested.join("src");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    git(
        &parent,
        &[
            "submodule",
            "add",
            "--quiet",
            upstream.to_str().unwrap(),
            "nested",
        ],
    );
    std::fs::create_dir(&below_nested).unwrap();
    assert_eq!(
        git_toplevel(&below_nested),
        nested,
        "git must select the submodule, not the superproject"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_at(
        resolve_repository_root(&nested),
        &nested,
        "linked-worktree layout",
    );
    assert_refused_at(
        resolve_repository_root(&below_nested),
        &nested,
        "linked-worktree layout",
    );
}

#[test]
fn refuses_a_nested_linked_worktree_with_a_mismatched_backlink_instead_of_its_parent() {
    let (_fixture, root) = fixture_root();
    let owner = root.join("owner");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let below_nested = nested.join("src");
    repo_with_a_commit(&owner);
    init_git_repo(&parent);
    git(
        &owner,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            nested.to_str().unwrap(),
        ],
    );
    std::fs::create_dir(&below_nested).unwrap();
    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);

    std::fs::write(
        owner.join(".git/worktrees/nested/gitdir"),
        format!("{}\n", parent.join("elsewhere/.git").display()),
    )
    .unwrap();
    assert_eq!(
        git_toplevel(&below_nested),
        nested,
        "git must still select the linked worktree, not the parent"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_at(resolve_repository_root(&nested), &nested, "backlink");
    assert_refused_at(resolve_repository_root(&below_nested), &nested, "backlink");
}

#[test]
fn resolves_past_a_marker_git_itself_skips_to_the_repository_git_selects() {
    let (_fixture, root) = fixture_root();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    std::fs::create_dir_all(nested.join(".git")).unwrap();
    assert_eq!(
        git_toplevel(&nested),
        parent,
        "git must skip a .git directory that is not a repository"
    );

    assert_eq!(resolve_repository_root(&nested).unwrap(), parent);
}
