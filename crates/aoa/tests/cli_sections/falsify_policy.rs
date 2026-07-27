use super::*;

// Criterion 7: falsify writes falsification.json with a verdict field.
#[test]
fn falsify_writes_verdict_file() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--build-meta")
        .arg(fixture("build_meta_ok.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let written = std::fs::read_to_string(&out).expect("falsification.json written");
    let parsed: Value = serde_json::from_str(&written).expect("valid json");
    assert!(parsed.get("verdict").is_some(), "missing verdict field");
}

// Abstain-safe default: WITHOUT --build-meta the convention inputs' provenance
// is unknown, so the gate treats them as degraded and abstains — omitting the
// build report can never silently read as "not degraded".
#[test]
fn falsify_without_build_meta_abstains_as_degraded() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .failure();

    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("written")).expect("json");
    assert_eq!(parsed["verdict"], "inconclusive");
    assert_eq!(parsed["precondition_unmet"], "convention_inputs_degraded");
    let notes = parsed["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .unwrap_or_default()
            .contains("abstain-safe default")),
        "the report must say the degradation is the missing build-meta default, got {notes:?}"
    );
}

// Criterion 8 (R-silent): an unsupported forge fails loudly, never a silent no-op.
#[test]
fn policy_compile_unknown_forge_fails_loudly() {
    aoa()
        .args(["policy", "compile", "--forge", "svn-hooks"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported forge"));
}

#[test]
fn policy_compile_known_forge_succeeds() {
    let repo = TempDir::new().unwrap();
    std::fs::write(repo.path().join("aoa-policy.yaml"), "protected_paths: []\n").unwrap();
    aoa()
        .args([
            "policy",
            "compile",
            "--repo",
            repo.path().to_str().unwrap(),
            "--forge",
            "github-actions",
        ])
        .assert()
        .success();
}

// A known forge but no aoa-policy.yaml fails loud — compiling from a missing
// policy is a user error, not a silent empty default.
#[test]
fn policy_compile_without_policy_file_fails_loud() {
    let repo = TempDir::new().unwrap();
    aoa()
        .args(["policy", "compile", "--repo", repo.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no policy file"));
}

pub(super) fn init_git_repo(path: &Path) {
    run_git(path, &["init", "-q"]);
    run_git(path, &["config", "user.email", "test@example.com"]);
    run_git(path, &["config", "user.name", "test"]);
}

pub(super) fn run_git(path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}
