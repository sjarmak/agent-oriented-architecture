use super::enforce::aoa_stdin;
use super::*;

const PROTECTIVE_POLICY: &str = "protected_paths: [\"src.rs\"]\nreproduction_required: false\n";
const PLANTED: &str = "planted before the check\n";

fn init_git_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    let status = Command::new("git")
        .args(["init", "--quiet", "--template="])
        .arg(path)
        .status()
        .expect("git is available for repository-boundary tests");
    assert!(status.success(), "git init failed for hook fixture");
}

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

fn write_to(target: &str, cwd: Option<&str>) -> String {
    let mut payload = serde_json::json!({
        "session_id": "it-relative-target",
        "tool_name": "Write",
        "tool_input": {"file_path": target},
    });
    if let Some(cwd) = cwd {
        payload["cwd"] = Value::String(cwd.into());
    }
    serde_json::to_string(&payload).unwrap()
}

fn protected_repository_with_a_subdirectory() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let fixture = TempDir::new().unwrap();
    let repo = fixture.path().canonicalize().unwrap().join("O");
    init_git_repo(&repo);
    std::fs::write(repo.join("aoa-policy.yaml"), PROTECTIVE_POLICY).unwrap();
    let target = repo.join("src.rs");
    plant(&target);
    let sub = repo.join("sub");
    std::fs::create_dir(&sub).unwrap();
    (fixture, repo, sub, target)
}

#[test]
fn enforce_check_anchors_a_relative_target_at_the_hook_cwd_not_the_repository_root() {
    let (_fixture, repo, sub, target) = protected_repository_with_a_subdirectory();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to("../src.rs", sub.to_str()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "protected path: policy forbids writing 'src.rs'",
        ))
        .stderr(predicate::str::contains(format!(
            "(enforced for {})",
            repo.display()
        )));
    assert_planted_target_is_untouched(&target);
}

#[test]
fn enforce_check_anchors_a_relative_target_from_the_repository_root_as_before() {
    let (_fixture, repo, _sub, target) = protected_repository_with_a_subdirectory();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to("src.rs", repo.to_str()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "protected path: policy forbids writing 'src.rs'",
        ))
        .stderr(predicate::str::contains(format!(
            "(enforced for {})",
            repo.display()
        )));
    assert_planted_target_is_untouched(&target);

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_to("docs/notes.md", repo.to_str()))
        .assert()
        .success();
}

#[test]
fn enforce_check_refuses_a_relative_target_when_the_hook_has_no_usable_cwd() {
    let (_fixture, repo, _sub, target) = protected_repository_with_a_subdirectory();
    let elsewhere = TempDir::new().unwrap();

    for cwd in [None, Some(""), Some("O/sub")] {
        aoa_stdin()
            .args(["enforce", "check"])
            .current_dir(&repo)
            .write_stdin(write_to("src.rs", cwd))
            .assert()
            .code(2)
            .stderr(predicate::str::contains("hook cwd"))
            .stderr(predicate::str::contains("src.rs"));
        aoa_stdin()
            .args(["enforce", "check"])
            .current_dir(elsewhere.path())
            .write_stdin(write_to("src.rs", cwd))
            .assert()
            .code(2)
            .stderr(predicate::str::contains("hook cwd"));
    }
    assert_planted_target_is_untouched(&target);
    assert!(
        !repo.join(".aoa").exists(),
        "a write the gate could not anchor leaves no record"
    );
}
