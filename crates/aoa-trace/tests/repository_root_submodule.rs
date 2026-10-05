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

fn add_submodule(superproject: &Path, upstream: &Path, name: &str) {
    git(
        superproject,
        &[
            "submodule",
            "add",
            "--quiet",
            upstream.to_str().unwrap(),
            name,
        ],
    );
}

fn set_core_worktree(git_dir: &Path, worktree: &Path) {
    git(
        git_dir,
        &[
            "config",
            "--file",
            git_dir.join("config").to_str().unwrap(),
            "core.worktree",
            worktree.to_str().unwrap(),
        ],
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

fn superproject_with_submodule(root: &Path) -> (PathBuf, PathBuf) {
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "nested");
    let nested = parent.join("nested");
    (parent, nested)
}

#[test]
fn resolves_a_healthy_submodule_to_itself_not_its_superproject() {
    let (_fixture, root) = fixture_root();
    let (parent, nested) = superproject_with_submodule(&root);
    let below_nested = nested.join("src");
    std::fs::create_dir(&below_nested).unwrap();
    assert_eq!(
        git_toplevel(&below_nested),
        nested,
        "git must select the submodule, not the superproject"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);
    assert_eq!(resolve_repository_root(&below_nested).unwrap(), nested);
}

#[test]
fn resolves_a_submodule_below_an_intermediate_directory_of_its_superproject() {
    let (_fixture, root) = fixture_root();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("vendor/libs/nested");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "vendor/libs/nested");

    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);
    assert_eq!(
        resolve_repository_root(&parent.join("vendor/libs")).unwrap(),
        parent
    );
}

#[test]
fn resolves_a_submodule_of_a_linked_worktree_superproject_to_itself() {
    let (_fixture, root) = fixture_root();
    let (parent, _) = superproject_with_submodule(&root);
    git(&parent, &["commit", "--quiet", "-m", "add the submodule"]);
    let linked = root.join("linked");
    git(
        &parent,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            linked.to_str().unwrap(),
        ],
    );
    git(&linked, &["submodule", "update", "--quiet", "--init"]);
    let nested = linked.join("nested");
    assert_eq!(git_toplevel(&nested), nested);

    assert_eq!(resolve_repository_root(&linked).unwrap(), linked);
    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);
}

#[test]
fn refuses_a_marker_file_naming_a_planted_gitdir_outside_the_superproject_modules() {
    let (_fixture, root) = fixture_root();
    let planted = root.join("planted");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let below_nested = nested.join("src");
    init_git_repo(&planted);
    init_git_repo(&parent);
    std::fs::create_dir_all(&below_nested).unwrap();
    std::fs::write(
        nested.join(".git"),
        format!("gitdir: {}\n", planted.join(".git").display()),
    )
    .unwrap();
    assert_eq!(
        git_toplevel(&below_nested),
        nested,
        "git must root the planted repository at the marker's directory"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_at(
        resolve_repository_root(&nested),
        &nested,
        "submodule layout",
    );
    assert_refused_at(
        resolve_repository_root(&below_nested),
        &nested,
        "submodule layout",
    );

    set_core_worktree(&planted.join(".git"), &nested);
    assert_eq!(git_toplevel(&below_nested), nested);
    assert_refused_at(
        resolve_repository_root(&nested),
        &nested,
        "submodule layout",
    );
}

#[test]
fn refuses_a_marker_file_naming_a_separate_gitdir_with_no_enclosing_repository() {
    let (_fixture, root) = fixture_root();
    let worktree = root.join("worktree");
    std::fs::create_dir(&worktree).unwrap();
    git(
        &worktree,
        &[
            "init",
            "--quiet",
            "--separate-git-dir",
            root.join("separate.git").to_str().unwrap(),
        ],
    );
    assert_eq!(git_toplevel(&worktree), worktree);

    assert_refused_at(
        resolve_repository_root(&worktree),
        &worktree,
        "submodule layout",
    );
}

#[test]
fn refuses_a_submodule_gitdir_whose_core_worktree_names_another_directory() {
    let (_fixture, root) = fixture_root();
    let (parent, nested) = superproject_with_submodule(&root);
    let below_nested = nested.join("src");
    let other = root.join("other");
    std::fs::create_dir(&below_nested).unwrap();
    std::fs::create_dir(&other).unwrap();
    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);

    set_core_worktree(&parent.join(".git/modules/nested"), &other);
    assert_eq!(
        git_toplevel(&below_nested),
        other,
        "git must root the submodule's repository where core.worktree names"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_at(
        resolve_repository_root(&nested),
        &nested,
        "instead of candidate",
    );
    assert_refused_at(
        resolve_repository_root(&below_nested),
        &nested,
        "instead of candidate",
    );
}

#[test]
fn refuses_a_submodule_gitdir_that_states_no_core_worktree() {
    let (_fixture, root) = fixture_root();
    let (parent, nested) = superproject_with_submodule(&root);
    let git_dir = parent.join(".git/modules/nested");
    git(
        &git_dir,
        &[
            "config",
            "--file",
            git_dir.join("config").to_str().unwrap(),
            "--unset",
            "core.worktree",
        ],
    );
    assert_eq!(
        git_toplevel(&nested),
        nested,
        "git must fall back to the marker's directory"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_at(resolve_repository_root(&nested), &nested, "core.worktree");
}

#[test]
fn refuses_a_submodule_whose_config_file_names_another_directory_than_git_uses() {
    let (_fixture, root) = fixture_root();
    let (parent, nested) = superproject_with_submodule(&root);
    let git_dir = parent.join(".git/modules/nested");
    let other = root.join("other");
    std::fs::create_dir(&other).unwrap();
    set_core_worktree(&git_dir, &other);
    git(&git_dir, &["config", "extensions.worktreeConfig", "true"]);
    std::fs::write(
        git_dir.join("config.worktree"),
        format!("[core]\n\tworktree = {}\n", nested.display()),
    )
    .unwrap();
    assert_eq!(
        git_toplevel(&nested),
        nested,
        "git must take the worktree from the worktree-specific configuration"
    );

    assert_eq!(resolve_repository_root(&parent).unwrap(), parent);
    assert_refused_at(
        resolve_repository_root(&nested),
        &nested,
        "does not resolve to",
    );
}

#[test]
fn resolves_a_submodule_of_a_submodule_to_itself_only_while_its_superproject_validates() {
    let (_fixture, root) = fixture_root();
    let innermost_upstream = root.join("innermost-upstream");
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let innermost = nested.join("innermost");
    let below_innermost = innermost.join("src");
    let other = root.join("other");
    repo_with_a_commit(&innermost_upstream);
    init_git_repo(&upstream);
    add_submodule(&upstream, &innermost_upstream, "innermost");
    git(&upstream, &["commit", "--quiet", "-m", "add the submodule"]);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "nested");
    git(
        &parent,
        &["submodule", "update", "--quiet", "--init", "--recursive"],
    );
    std::fs::create_dir(&below_innermost).unwrap();
    std::fs::create_dir(&other).unwrap();
    assert_eq!(git_toplevel(&below_innermost), innermost);

    assert_eq!(resolve_repository_root(&nested).unwrap(), nested);
    assert_eq!(resolve_repository_root(&innermost).unwrap(), innermost);
    assert_eq!(
        resolve_repository_root(&below_innermost).unwrap(),
        innermost
    );

    set_core_worktree(&parent.join(".git/modules/nested"), &other);
    assert_eq!(
        git_toplevel(&below_innermost),
        innermost,
        "git must still select the innermost submodule"
    );
    assert_refused_at(
        resolve_repository_root(&innermost),
        &innermost,
        "submodule layout",
    );
    assert_refused_at(
        resolve_repository_root(&below_innermost),
        &innermost,
        "submodule layout",
    );
}
