use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn aoa() -> Command {
    Command::cargo_bin("aoa").expect("aoa binary builds")
}

// Criterion 2: validate-trace exits 0 + prints per-type counts for a valid
// trace, and exits non-zero for an invalid one.
#[test]
fn validate_trace_valid_prints_counts_and_exits_zero() {
    aoa()
        .args(["eval", "--validate-trace"])
        .arg(fixture("valid_trace.json"))
        .assert()
        .success()
        .stdout(predicate::str::contains("file.read"))
        .stdout(predicate::str::contains("retrieval.search"));
}

#[test]
fn validate_trace_invalid_exits_non_zero() {
    aoa()
        .args(["eval", "--validate-trace"])
        .arg(fixture("invalid_trace.json"))
        .assert()
        .failure();
}

// Criterion 9 (eval half): --json yields parseable JSON; default yields human text.
#[test]
fn validate_trace_json_is_parseable() {
    let output = aoa()
        .args(["eval", "--json", "--validate-trace"])
        .arg(fixture("valid_trace.json"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["total"], 5);
}

// Criterion 3: compare prints the reward-hacking gap delta.
#[test]
fn compare_prints_gap_delta() {
    aoa()
        .args(["eval", "--compare"])
        .arg(fixture("baseline.json"))
        .arg(fixture("migrated.json"))
        .assert()
        .success()
        .stdout(predicate::str::contains("gap delta"));
}

#[test]
fn compare_json_carries_gap_delta() {
    let output = aoa()
        .args(["eval", "--json", "--compare"])
        .arg(fixture("baseline.json"))
        .arg(fixture("migrated.json"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("gap_delta").is_some());
    assert_eq!(parsed["label"], "good");
}

// Criterion 4: observe makes no tracked-file changes.
#[test]
fn observe_makes_no_tracked_changes() {
    let repo = TempDir::new().expect("tempdir");
    init_git_repo(repo.path());

    aoa()
        .args(["observe", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let status = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    let porcelain = String::from_utf8_lossy(&status.stdout);
    // The only artifact is the explicitly-ignored .aoa/ tree, which carries its
    // own ignore guard, so the working tree stays clean.
    assert!(
        porcelain.trim().is_empty(),
        "working tree not clean: {porcelain}"
    );
}

// Criterion 5 + 9 (audit half): tiered punch-list, --json structured, --fail-on tier1.
#[test]
fn audit_human_prints_punch_list() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("punch-list"))
        .stdout(predicate::str::contains("tier-1"));
}

#[test]
fn audit_json_is_parseable() {
    let repo = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed["items"].is_array());
}

#[test]
fn audit_fail_on_tier1_exits_non_zero_when_tier1_present() {
    // A bare repo is missing the runtime-hook and CI planes (both Tier-1).
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure();
}

#[test]
fn audit_fail_on_tier1_exits_zero_without_tier1_gap() {
    // Present the two Tier-1 planes (runtime hook + CI) so only the Tier-2
    // pre-commit plane is missing; --fail-on tier1 must then exit 0.
    let repo = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(repo.path().join(".claude/settings.json"), "{}").unwrap();
    std::fs::create_dir_all(repo.path().join(".github/workflows")).unwrap();

    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

#[test]
fn audit_without_fail_on_exits_zero_even_with_tier1_gap() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

// Criterion 6: lint-context --changed flags only changed files and honors the
// oversized-context suppression marker.
#[test]
fn lint_context_changed_filters_and_honors_suppression() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("AGENTS.md");
    let changed = dir.path().join("changed.md");
    let other = dir.path().join("other.md");
    let suppressed = dir.path().join("suppressed.md");

    std::fs::write(
        &root,
        "# Root\n\nSee [changed](changed.md), [other](other.md), [suppressed](suppressed.md).\n",
    )
    .unwrap();

    let dup_section = format!("# Dup\n\nbody\n\n# Dup\n\n{}", "line\n".repeat(50));
    std::fs::write(&changed, &dup_section).unwrap();
    std::fs::write(&other, &dup_section).unwrap();
    std::fs::write(
        &suppressed,
        "# aoa-allow: oversized-context giant onboarding doc\n\n# Suppressed\n\nbody\n",
    )
    .unwrap();

    let output = aoa()
        .args(["lint-context", "--json", "--root"])
        .arg(&root)
        .arg("--changed")
        .arg(&changed)
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    let findings = parsed["findings"].as_array().expect("findings array");
    assert!(
        !findings.is_empty(),
        "expected findings for the changed file"
    );
    for finding in findings {
        let file = finding["file"].as_str().unwrap();
        assert!(
            file.ends_with("changed.md"),
            "finding leaked from a non-changed file: {file}"
        );
        assert!(
            !file.ends_with("other.md"),
            "finding leaked from other.md: {file}"
        );
    }

    let suppressions = parsed["suppressed"].as_array().expect("suppressed array");
    assert!(
        suppressions
            .iter()
            .any(|s| s["file"].as_str().unwrap().ends_with("suppressed.md")),
        "suppression marker not honored"
    );
}

#[test]
fn lint_context_human_renders_text() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("AGENTS.md");
    std::fs::write(&root, "# Root\n\nplain doc with no smells\n").unwrap();

    aoa()
        .args(["lint-context", "--root"])
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("context lint"));
}

// Criterion 7: falsify writes falsification.json with a verdict field.
#[test]
fn falsify_writes_verdict_file() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let written = std::fs::read_to_string(&out).expect("falsification.json written");
    let parsed: Value = serde_json::from_str(&written).expect("valid json");
    assert!(parsed.get("verdict").is_some(), "missing verdict field");
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
    aoa()
        .args(["policy", "compile", "--forge", "github-actions"])
        .assert()
        .success();
}

fn init_git_repo(path: &Path) {
    run_git(path, &["init", "-q"]);
    run_git(path, &["config", "user.email", "test@example.com"]);
    run_git(path, &["config", "user.name", "test"]);
}

fn run_git(path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}
