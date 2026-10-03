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
    // The runtime plane is made *live* rather than merely registered, so the
    // fixture pins the strongest passing state. The two weaker ones are not
    // interchangeable with it: a traces directory that exists and stays empty is
    // a Tier-1 silence (aoa-dpluh) and would fail this gate, while an absent one
    // is the Tier-3 unobserved plane (aoa-rsixa) and would pass it — but pass on
    // a different fact than the one this test is pinning.
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

const LINT_DUPLICATED_HEADING: &str = "# Rules\n\nfirst\n\n# Rules\n\nsecond\n";

fn lint_json(dir: &Path, args: &[&str]) -> Value {
    let output = aoa()
        .current_dir(dir)
        .args(["lint-context", "--json"])
        .args(args)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "lint-context failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

fn json_strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|entry| entry.as_str().expect("string"))
        .collect()
}

fn finding_files(parsed: &Value) -> Vec<&str> {
    parsed["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|finding| finding["file"].as_str().expect("file"))
        .collect()
}

#[test]
fn lint_context_without_root_lints_a_claude_only_tree_with_nested_files() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("CLAUDE.md"), "# Root\n\nplain\n").unwrap();
    std::fs::create_dir_all(dir.path().join("services/api")).unwrap();
    std::fs::write(
        dir.path().join("services/api/CLAUDE.md"),
        LINT_DUPLICATED_HEADING,
    )
    .unwrap();

    let parsed = lint_json(dir.path(), &[]);

    assert_eq!(
        json_strings(&parsed["roots"]),
        ["CLAUDE.md", "services/api/CLAUDE.md"]
    );
    assert_eq!(finding_files(&parsed), ["services/api/CLAUDE.md"]);
}

#[test]
fn lint_context_explicit_root_also_lints_nested_context_files() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("CLAUDE.md"), "# Root\n\nplain\n").unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/AGENTS.md"), LINT_DUPLICATED_HEADING).unwrap();

    let parsed = lint_json(dir.path(), &["--root", "./CLAUDE.md"]);

    assert_eq!(
        json_strings(&parsed["roots"]),
        ["CLAUDE.md", "pkg/AGENTS.md"]
    );
    assert_eq!(finding_files(&parsed), ["pkg/AGENTS.md"]);
}

#[test]
fn lint_context_human_reports_how_many_roots_were_linted() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Root\n").unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/CLAUDE.md"), "# Pkg\n").unwrap();

    aoa()
        .current_dir(dir.path())
        .arg("lint-context")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "context lint: 0 finding(s) across 2 context root(s)",
        ));
}

#[test]
fn lint_context_without_any_context_file_says_how_to_name_one() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("README.md"), "# Readme\n").unwrap();

    aoa()
        .current_dir(dir.path())
        .arg("lint-context")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no AGENTS.md or CLAUDE.md found"))
        .stderr(predicate::str::contains("--root"));
}

#[test]
fn lint_context_missing_explicit_root_states_the_cause_once() {
    let dir = TempDir::new().expect("tempdir");

    let output = aoa()
        .current_dir(dir.path())
        .args(["lint-context", "--root", "AGENTS.md"])
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr
            .matches("failed to read context file AGENTS.md")
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(stderr.matches("os error").count(), 1, "{stderr}");
}
