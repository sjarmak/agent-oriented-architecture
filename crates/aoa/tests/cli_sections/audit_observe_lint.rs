use super::falsify_policy::init_git_repo;
use super::*;

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

// aoa-d6t.31 review follow-up: a repo whose workspace manifest is malformed
// must still get its full punch-list — the CLI degrades to repo-wide findings
// with the discovery failure surfaced, never an abort with no report.
#[test]
fn audit_degrades_on_malformed_workspace_manifest() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(repo.path().join("package.json"), "{ \"name\": \"x\", }").unwrap();
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success(), "audit must not abort: {output:?}");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(!parsed["items"].as_array().expect("items").is_empty());
    assert!(parsed["subtree_discovery_warning"]
        .as_str()
        .expect("warning surfaced on the wire")
        .contains("package.json"));
}

#[test]
fn recommend_degrades_on_malformed_workspace_manifest() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(repo.path().join("package.json"), "{ \"name\": \"x\", }").unwrap();
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();

    let output = aoa()
        .args(["recommend", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "recommend must not abort: {output:?}"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(!parsed["items"].as_array().expect("items").is_empty());
    // The recommendation report has no warning field of its own; the CLI
    // surfaces the audit's degradation on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("package.json"),
        "discovery failure must be surfaced on stderr: {stderr}"
    );
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
    //
    // The runtime plane has to be *live*, not merely installed: an installed
    // hook set that has emitted nothing is itself a Tier-1 finding (aoa-dpluh),
    // so a fixture that only writes settings.json no longer describes a repo
    // with no Tier-1 gap.
    let repo = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{"hooks":{
            "PostToolUse":[{"hooks":[
                {"command":"aoa enforce record"},
                {"command":"aoa enforce commit"}
            ]}],
            "PreToolUse":[{"hooks":[{"command":"aoa enforce check"}]}],
            "PostToolUseFailure":[{"hooks":[{"command":"aoa enforce fail"}]}],
            "PermissionDenied":[{"hooks":[{"command":"aoa enforce deny"}]}]
        }}"#,
    )
    .unwrap();
    std::fs::create_dir_all(repo.path().join(".github/workflows")).unwrap();
    std::fs::create_dir_all(repo.path().join(".aoa/traces")).unwrap();
    std::fs::write(
        repo.path().join(".aoa/traces/live-s1.jsonl"),
        "{\"type\":\"test.run\",\"source\":\"native\",\"seq\":0,\"attributes\":{}}\n",
    )
    .unwrap();

    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

#[test]
fn audit_fail_on_tier1_rejects_a_gutted_runtime_hook_file() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(repo.path().join(".claude/settings.json"), "{}").unwrap();
    std::fs::create_dir_all(repo.path().join(".github/workflows")).unwrap();

    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "missing enforcement plane: runtime hook",
        ));
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
