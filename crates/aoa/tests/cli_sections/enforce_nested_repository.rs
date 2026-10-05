use super::enforce::aoa_stdin;
use super::*;

const PERMISSIVE_POLICY: &str = "reproduction_required: false\n";
const PROTECTIVE_POLICY: &str = "protected_paths: [\"src/**\"]\nreproduction_required: false\n";

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
    git(path, &["init", "--quiet", "--template="]);
}

fn repo_with_a_commit(path: &Path) {
    init_git_repo(path);
    git(
        path,
        &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
    );
}

fn write_to_src_from(cwd: &Path) -> String {
    serde_json::to_string(&serde_json::json!({
        "session_id": "it-unvalidated-nested",
        "tool_name": "Write",
        "tool_input": {"file_path": "src/lib.rs"},
        "cwd": cwd.to_str().unwrap(),
    }))
    .unwrap()
}

fn assert_nested_write_is_refused_without_the_parent_policy(
    parent: &Path,
    nested: &Path,
    why: &str,
) {
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(nested))
        .assert()
        .code(2)
        .stderr(predicate::str::contains(why))
        .stderr(predicate::str::contains(
            nested.join(".git").to_str().unwrap(),
        ));
    assert!(
        !parent.join(".aoa").exists(),
        "a nested repository that cannot be validated must not be enforced as its parent"
    );
    assert!(!nested.join(".aoa").exists());

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(parent))
        .assert()
        .success();
    assert!(
        parent.join(".aoa").exists(),
        "the fixture must leave the parent's permissive policy in force for the parent itself"
    );
}

#[test]
fn enforce_check_refuses_a_nested_submodule_instead_of_applying_its_superproject_policy() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
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

    assert_nested_write_is_refused_without_the_parent_policy(
        &parent,
        &nested,
        "linked-worktree layout",
    );
}

#[test]
fn enforce_check_refuses_a_nested_worktree_with_a_mismatched_backlink_instead_of_applying_its_parent_policy(
) {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let owner = root.join("owner");
    let parent = root.join("parent");
    let nested = parent.join("nested");
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
    std::fs::write(
        owner.join(".git/worktrees/nested/gitdir"),
        format!("{}\n", parent.join("elsewhere/.git").display()),
    )
    .unwrap();

    assert_nested_write_is_refused_without_the_parent_policy(&parent, &nested, "backlink");
}
