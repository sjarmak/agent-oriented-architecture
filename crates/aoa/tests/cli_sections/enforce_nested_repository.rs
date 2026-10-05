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

fn assert_write_to_src_is_blocked_by_the_policy_of(governing: &Path, bystanders: &[&Path]) {
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(governing))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("protected path"));
    for bystander in bystanders {
        assert!(
            !bystander.join(".aoa").exists(),
            "{} must not be enforced in place of {}",
            bystander.display(),
            governing.display()
        );
    }
}

#[test]
fn enforce_check_applies_a_submodules_own_policy_not_its_superprojects() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "nested");
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();

    assert_write_to_src_is_blocked_by_the_policy_of(&nested, &[&parent]);

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(&parent))
        .assert()
        .success();
}

#[test]
fn enforce_check_applies_the_superprojects_policy_to_the_superproject_not_its_submodules() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "nested");
    std::fs::write(parent.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(&nested))
        .assert()
        .success();
    assert!(
        !parent.join(".aoa").exists(),
        "a submodule must not be enforced as its superproject"
    );
    assert_write_to_src_is_blocked_by_the_policy_of(&parent, &[]);
}

#[test]
fn enforce_check_applies_the_policy_of_a_submodule_of_a_submodule_not_either_superproject() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let innermost_upstream = root.join("innermost-upstream");
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let innermost = nested.join("innermost");
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
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(innermost.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();

    assert_write_to_src_is_blocked_by_the_policy_of(&innermost, &[&parent, &nested]);
}

#[test]
fn enforce_check_refuses_a_marker_file_naming_a_planted_gitdir_instead_of_applying_any_policy() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let planted = root.join("planted");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&planted);
    set_core_worktree(&planted.join(".git"), &nested);
    init_git_repo(&parent);
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(
        nested.join(".git"),
        format!("gitdir: {}\n", planted.join(".git").display()),
    )
    .unwrap();

    assert_nested_write_is_refused_without_the_parent_policy(&parent, &nested, "submodule layout");
}

#[test]
fn enforce_check_refuses_a_submodule_whose_core_worktree_names_another_directory() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    let other = root.join("other");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "nested");
    init_git_repo(&other);
    std::fs::write(other.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    set_core_worktree(&parent.join(".git/modules/nested"), &other);

    assert_nested_write_is_refused_without_the_parent_policy(
        &parent,
        &nested,
        "instead of candidate",
    );
    assert!(
        !other.join(".aoa").exists(),
        "the directory core.worktree names must not be enforced in place of the submodule"
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

#[test]
fn enforce_check_refuses_a_marker_whose_repository_is_rooted_above_the_parent_instead_of_applying_the_parent_policy(
) {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let elsewhere = root.join("elsewhere");
    let outer = root.join("outer");
    let parent = outer.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&elsewhere);
    git(
        &elsewhere,
        &["config", "core.worktree", outer.to_str().unwrap()],
    );
    init_git_repo(&parent);
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(
        nested.join(".git"),
        format!("gitdir: {}\n", elsewhere.join(".git").display()),
    )
    .unwrap();

    assert_nested_write_is_refused_without_the_parent_policy(
        &parent,
        &nested,
        "instead of candidate",
    );
}

fn assert_enforce_check_refuses_cwd_without_the_parent_policy(parent: &Path, cwd: &Path) {
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(cwd))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--show-toplevel"))
        .stderr(predicate::str::contains(format!(
            "validation failed for {}:",
            cwd.display()
        )));
    assert!(
        !parent.join(".aoa").exists(),
        "a directory git does not root at the parent must not be enforced as the parent"
    );
}

const NESTED_SRC_PROTECTED_BY_PARENT: &str =
    "protected_paths: [\"nested/src/**\"]\nreproduction_required: false\n";
const PLANTED: &str = "planted before the check\n";

fn plant(target: &Path) {
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, PLANTED).unwrap();
}

fn assert_planted_target_is_untouched(target: &Path) {
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        PLANTED,
        "{} must not be modified by a refused write",
        target.display()
    );
}

fn write_to(target: &str, cwd: &Path) -> String {
    serde_json::to_string(&serde_json::json!({
        "session_id": "it-nested-policy",
        "tool_name": "Write",
        "tool_input": {"file_path": target},
        "cwd": cwd.to_str().unwrap(),
    }))
    .unwrap()
}

fn assert_check_is_refused_by_the_policy_of(root: &Path, payload: String, policy: &str) {
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(payload)
        .assert()
        .code(2)
        .stderr(predicate::str::contains(policy))
        .stderr(predicate::str::contains(format!(
            "(enforced for {})",
            root.display()
        )));
}

#[test]
fn enforce_check_holds_a_parents_refusal_inside_a_nested_repository_with_its_own_policy() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(
        parent.join("aoa-policy.yaml"),
        NESTED_SRC_PROTECTED_BY_PARENT,
    )
    .unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    assert_check_is_refused_by_the_policy_of(
        &parent,
        write_to_src_from(&nested),
        "protected path: policy forbids writing 'nested/src/lib.rs'",
    );
    assert_planted_target_is_untouched(&target);
    assert!(
        !parent.join(".aoa").exists(),
        "the refusal is recorded in the session's own repository, not the enclosing one"
    );

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to("docs/notes.md", &nested))
        .assert()
        .success();
}

#[test]
fn enforce_check_holds_a_superprojects_refusal_inside_a_submodule_with_its_own_policy() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let upstream = root.join("upstream");
    let parent = root.join("parent");
    let nested = parent.join("nested");
    repo_with_a_commit(&upstream);
    init_git_repo(&parent);
    add_submodule(&parent, &upstream, "nested");
    std::fs::write(
        parent.join("aoa-policy.yaml"),
        NESTED_SRC_PROTECTED_BY_PARENT,
    )
    .unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    assert_check_is_refused_by_the_policy_of(
        &parent,
        write_to_src_from(&nested),
        "protected path: policy forbids writing 'nested/src/lib.rs'",
    );
    assert_planted_target_is_untouched(&target);
}

#[cfg(unix)]
#[test]
fn enforce_check_lets_a_nested_repository_named_one_space_add_a_refusal_its_parent_does_not_make() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join(" ");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    assert_check_is_refused_by_the_policy_of(
        &nested,
        write_to_src_from(&nested),
        "protected path: policy forbids writing 'src/lib.rs'",
    );
    assert_planted_target_is_untouched(&target);
    assert!(
        !parent.join(".aoa").exists(),
        "a nested repository must not be enforced as its parent"
    );
}

#[test]
fn enforce_check_holds_a_parents_refusal_for_a_target_the_nested_repository_does_not_contain() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(parent.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = parent.join("src/lib.rs");
    plant(&target);

    let elsewhere = root.join("elsewhere/notes.md");
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to(elsewhere.to_str().unwrap(), &nested))
        .assert()
        .success();
    assert!(
        !nested.join(".aoa").exists() && !parent.join(".aoa").exists(),
        "a target no governed repository contains leaves no record"
    );

    for spelling in ["../src/lib.rs", target.to_str().unwrap()] {
        assert_check_is_refused_by_the_policy_of(
            &parent,
            write_to(spelling, &nested),
            "protected path: policy forbids writing 'src/lib.rs'",
        );
    }
    assert_planted_target_is_untouched(&target);
    assert!(
        !parent.join(".aoa").exists(),
        "the refusal is recorded in the session's own repository, not the enclosing one"
    );
}

#[test]
fn enforce_check_holds_a_parents_reproduction_requirement_a_nested_policy_switches_off() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(parent.join("aoa-policy.yaml"), "protected_paths: []\n").unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    assert_check_is_refused_by_the_policy_of(
        &parent,
        write_to_src_from(&nested),
        "reproduction-before-mutation",
    );
    assert_planted_target_is_untouched(&target);
}

#[test]
fn enforce_check_lets_an_enclosing_repository_without_a_policy_contribute_nothing() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(&nested))
        .assert()
        .success();
}

#[test]
fn enforce_check_refuses_a_nested_repository_whose_enclosing_repository_git_refuses() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    std::fs::create_dir_all(parent.join(".git")).unwrap();
    std::fs::write(parent.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(parent.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    init_git_repo(&nested);
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(&nested))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("validation failed for"))
        .stderr(predicate::str::contains(
            parent.join(".git").to_str().unwrap(),
        ));
    assert_planted_target_is_untouched(&target);
}

#[test]
fn enforce_check_holds_a_session_in_the_parent_to_a_refusal_the_nested_repository_adds() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    for (spelling, relative) in [
        ("nested/src/lib.rs", "src/lib.rs"),
        (target.to_str().unwrap(), "src/lib.rs"),
        ("nested/src/not/yet/made.rs", "src/not/yet/made.rs"),
    ] {
        assert_check_is_refused_by_the_policy_of(
            &nested,
            write_to(spelling, &parent),
            &format!("protected path: policy forbids writing '{relative}'"),
        );
    }
    assert_planted_target_is_untouched(&target);
    assert!(
        !nested.join(".aoa").exists(),
        "the refusal is recorded in the session's own repository, not the nested one"
    );

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to("nested/docs/notes.md", &parent))
        .assert()
        .success();
}

#[test]
fn enforce_check_leaves_a_governed_repository_the_session_is_not_inside_out_of_scope() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let session = root.join("session");
    let unrelated = root.join("unrelated");
    init_git_repo(&session);
    init_git_repo(&unrelated);
    std::fs::write(session.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(unrelated.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to(
            unrelated.join("src/lib.rs").to_str().unwrap(),
            &session,
        ))
        .assert()
        .success();
    assert!(
        !session.join(".aoa").exists() && !unrelated.join(".aoa").exists(),
        "a target outside every repository enclosing the session leaves no record"
    );
}

#[cfg(unix)]
#[test]
fn enforce_check_refuses_a_write_through_a_linked_directory_at_the_location_it_lands_in() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    init_git_repo(&parent);
    std::fs::write(
        parent.join("aoa-policy.yaml"),
        "protected_paths: [\"real/src/**\"]\nreproduction_required: false\n",
    )
    .unwrap();
    let target = parent.join("real/src/lib.rs");
    plant(&target);
    std::os::unix::fs::symlink(parent.join("real"), parent.join("nested")).unwrap();

    assert_check_is_refused_by_the_policy_of(
        &parent,
        write_to("nested/src/lib.rs", &parent),
        "protected path: policy forbids writing 'real/src/lib.rs'",
    );
    assert_planted_target_is_untouched(&target);
}

#[cfg(unix)]
#[test]
fn enforce_check_holds_a_write_through_a_link_to_the_policy_of_the_repository_it_lands_in() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let elsewhere = root.join("elsewhere");
    init_git_repo(&parent);
    init_git_repo(&elsewhere);
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    std::fs::write(elsewhere.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    let target = elsewhere.join("src/lib.rs");
    plant(&target);
    std::os::unix::fs::symlink(&elsewhere, parent.join("linked")).unwrap();

    assert_check_is_refused_by_the_policy_of(
        &elsewhere,
        write_to("linked/src/lib.rs", &parent),
        "protected path: policy forbids writing 'src/lib.rs'",
    );
    assert_planted_target_is_untouched(&target);
}

#[cfg(unix)]
fn assert_check_refuses_the_linked_policy_of(root: &Path, payload: String) {
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(payload)
        .assert()
        .code(2)
        .stderr(predicate::str::contains(format!(
            "cannot read policy at {}",
            root.join("aoa-policy.yaml").display()
        )));
}

#[cfg(unix)]
#[test]
fn enforce_check_refuses_an_enclosing_policy_that_is_a_symlink_to_a_permissive_one() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(root.join("permissive.yaml"), PERMISSIVE_POLICY).unwrap();
    std::os::unix::fs::symlink(root.join("permissive.yaml"), parent.join("aoa-policy.yaml"))
        .unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    assert_check_refuses_the_linked_policy_of(&parent, write_to_src_from(&nested));
    assert_planted_target_is_untouched(&target);
    assert_eq!(
        std::fs::read_to_string(root.join("permissive.yaml")).unwrap(),
        PERMISSIVE_POLICY
    );
}

#[cfg(unix)]
#[test]
fn enforce_check_refuses_an_enclosing_policy_that_is_a_dangling_symlink() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::os::unix::fs::symlink(root.join("absent.yaml"), parent.join("aoa-policy.yaml")).unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    assert_check_refuses_the_linked_policy_of(&parent, write_to_src_from(&nested));
    assert_planted_target_is_untouched(&target);
    assert!(
        !root.join("absent.yaml").exists(),
        "the dangling link's target must not be created"
    );
}

#[cfg(unix)]
#[test]
fn enforce_check_refuses_the_sessions_own_policy_when_it_is_a_symlink() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let repo = root.join("repo");
    init_git_repo(&repo);
    std::fs::write(root.join("permissive.yaml"), PERMISSIVE_POLICY).unwrap();
    std::os::unix::fs::symlink(root.join("permissive.yaml"), repo.join("aoa-policy.yaml")).unwrap();
    let target = repo.join("src/lib.rs");
    plant(&target);

    assert_check_refuses_the_linked_policy_of(&repo, write_to_src_from(&repo));
    assert_planted_target_is_untouched(&target);
}

#[test]
fn enforce_check_refuses_a_malformed_enclosing_policy() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let nested = parent.join("nested");
    init_git_repo(&parent);
    init_git_repo(&nested);
    std::fs::write(parent.join("aoa-policy.yaml"), "protected_paths: {\n").unwrap();
    std::fs::write(nested.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let target = nested.join("src/lib.rs");
    plant(&target);

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to_src_from(&nested))
        .assert()
        .code(2)
        .stderr(predicate::str::contains(format!(
            "invalid policy at {}",
            parent.join("aoa-policy.yaml").display()
        )));
    assert_planted_target_is_untouched(&target);
}

#[test]
fn enforce_check_refuses_a_nested_bare_repository_instead_of_applying_its_parent_policy() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let bare = parent.join("nested.git");
    init_git_repo(&parent);
    std::fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--quiet", "--bare", "--template="]);
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();

    assert_enforce_check_refuses_cwd_without_the_parent_policy(&parent, &bare);
    assert_enforce_check_refuses_cwd_without_the_parent_policy(&parent, &bare.join("objects"));
}

#[test]
fn enforce_check_refuses_a_cwd_inside_the_git_directory_of_the_root_itself() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    init_git_repo(&parent);
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();

    assert_enforce_check_refuses_cwd_without_the_parent_policy(&parent, &parent.join(".git"));
    assert_enforce_check_refuses_cwd_without_the_parent_policy(
        &parent,
        &parent.join(".git/objects"),
    );
}

#[test]
fn enforce_check_refuses_a_write_into_the_hooks_of_the_sessions_own_git_directory() {
    let fixture = TempDir::new().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    init_git_repo(&parent);
    std::fs::write(parent.join("aoa-policy.yaml"), PERMISSIVE_POLICY).unwrap();
    let hooks = parent.join(".git/hooks");
    let hook = hooks.join("pre-commit");
    plant(&hook);

    for target in [".git/hooks/pre-commit", ".git/hooks/post-checkout"] {
        aoa_stdin()
            .args(["enforce", "check"])
            .write_stdin(write_to(target, &parent))
            .assert()
            .code(2)
            .stderr(predicate::str::contains("--show-toplevel"))
            .stderr(predicate::str::contains(format!(
                "validation failed for {}:",
                hooks.display()
            )));
    }
    assert_planted_target_is_untouched(&hook);
    assert!(
        !hooks.join("post-checkout").exists(),
        "a refused write must not create the hook it was refused"
    );

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to("docs/notes.md", &parent))
        .assert()
        .success();
}
