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
        .args(["eval", "validate-trace"])
        .arg(fixture("valid_trace.json"))
        .assert()
        .success()
        .stdout(predicate::str::contains("file.read"))
        .stdout(predicate::str::contains("retrieval.search"));
}

#[test]
fn validate_trace_invalid_exits_non_zero() {
    aoa()
        .args(["eval", "validate-trace"])
        .arg(fixture("invalid_trace.json"))
        .assert()
        .failure();
}

// Criterion 9 (eval half): --json yields parseable JSON; default yields human text.
#[test]
fn validate_trace_json_is_parseable() {
    let output = aoa()
        .args(["eval", "validate-trace", "--json"])
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
        .args(["eval", "compare"])
        .arg(fixture("baseline.json"))
        .arg(fixture("migrated.json"))
        .assert()
        .success()
        .stdout(predicate::str::contains("gap delta"));
}

#[test]
fn compare_json_carries_gap_delta() {
    let output = aoa()
        .args(["eval", "compare", "--json"])
        .arg(fixture("baseline.json"))
        .arg(fixture("migrated.json"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("gap_delta").is_some());
    assert_eq!(parsed["label"], "good");
}

// --- aoa-2lw: eval run post-processes a codeprobe run -------------------------

fn run_dir() -> PathBuf {
    fixture("codeprobe_run")
}
fn tasks_dir() -> PathBuf {
    fixture("codeprobe_tasks")
}

// AC1 + AC4: emits a per-task record for each valid trial, and a per-trial error
// (non-zero exit) for BOTH a missing-scoring and a missing-transcript trial —
// never silently skipped.
#[test]
fn eval_run_emits_records_and_fails_loud_per_trial() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");
    // Two trials error (broken-no-scoring, broken-no-transcript) -> non-zero
    // exit, but the good records still computed.
    assert!(!output.status.success(), "broken trials must fail loud");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["record_count"], 2, "two good trials produce records");
    assert_eq!(parsed["error_count"], 2, "both broken trials are reported");

    let errors = parsed["errors"].as_array().expect("errors array");
    let err_for = |id: &str| {
        errors
            .iter()
            .find(|e| e["task_id"] == id)
            .unwrap_or_else(|| panic!("no error for {id}"))["error"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(
        err_for("broken-no-scoring").contains("scoring.json"),
        "missing-scoring error must name the root cause"
    );
    assert!(
        err_for("broken-no-transcript").contains("agent_output.txt"),
        "missing-transcript error must name the root cause"
    );

    // Every record carries the four metrics + the gap + conditioning.
    let records = parsed["records"].as_array().expect("records array");
    for rec in records {
        assert_eq!(rec["conditioned_on"], "held_out");
        assert_eq!(rec["visible_unobserved"], true);
        assert!(rec["retrieval_locality"].is_object());
        assert!(rec["invariant_discoverability"].is_object());
        assert!(rec["mutation_surface"].is_object());
        assert!(rec.get("gap").is_some());
        assert!(rec.get("transcript_warnings").is_some());
    }
}

// AC3: held-out drives counted_as_success; a held-out fail is not a success.
#[test]
fn eval_run_held_out_fail_not_counted_as_success() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let records = parsed["records"].as_array().unwrap();
    let by_id = |id: &str| records.iter().find(|r| r["task_id"] == id).unwrap().clone();

    let pass = by_id("external-filelist-000");
    assert_eq!(pass["held_out_success"], true);
    assert_eq!(pass["counted_as_success"], true);

    let fail = by_id("native-consensus-001");
    assert_eq!(fail["held_out_success"], false);
    assert_eq!(fail["counted_as_success"], false);
}

// AC2/AC3: provenance drives the gap (External -> available); edit-locality is
// reported null (never fabricated) when <2 accepted solutions exist.
#[test]
fn eval_run_gap_and_edit_locality_honor_provenance_and_solution_count() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let records = parsed["records"].as_array().unwrap();
    let by_id = |id: &str| records.iter().find(|r| r["task_id"] == id).unwrap().clone();

    // External provenance, single accepted solution.
    let ext = by_id("external-filelist-000");
    assert_eq!(ext["held_out_provenance"], "external");
    assert_eq!(ext["gap"]["status"], "available");
    assert!(
        ext["edit_locality"].is_null(),
        "1 solution -> no fabricated floor/ceiling"
    );
    assert!(ext["edit_locality_unavailable"]
        .as_str()
        .unwrap()
        .contains("insufficient"));

    // NativeComposed provenance, two accepted solutions -> edit-locality present.
    let nat = by_id("native-consensus-001");
    assert_eq!(nat["held_out_provenance"], "native_composed");
    assert_eq!(nat["gap"]["status"], "available");
    assert!(
        nat["edit_locality"].is_object(),
        "2 solutions -> edit-locality computed"
    );
}

// Without a graph source the symbol graph degrades to zero weight (logged),
// while records are still emitted (AC1).
#[test]
fn eval_run_degrades_graph_without_source() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let rec = parsed["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["task_id"] == "external-filelist-000")
        .unwrap()
        .clone();
    assert_eq!(rec["graph_quality"], "degraded");
    assert_eq!(rec["weight"], 0.0);
    assert_eq!(rec["repo_eligible_for_r0"], false);
    assert!(rec["graph_degrade_reason"]
        .as_str()
        .unwrap()
        .contains("no graph source"));
}

// Human (non-JSON) register renders text.
#[test]
fn eval_run_human_renders_text() {
    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .assert()
        .failure() // the scoring-less trial -> non-zero exit
        .stdout(predicate::str::contains("aoa eval run"))
        .stdout(predicate::str::contains("external-filelist-000"));
}

// --- aoa-d6t.26: per-subtree metric scoping in monorepos -----------------------

// A multi-member Cargo workspace repo enables automatic per-subtree metrics:
// the report carries the partition (additive fields), each record carries
// per-subtree rows, and the mode switch is logged to stderr, never silent.
#[test]
fn eval_run_emits_per_subtree_rows_for_multi_member_workspace() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--repo")
        .arg(fixture("subtree_repo"))
        .output()
        .expect("run");
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("per-subtree"),
        "automatic per-subtree mode must be logged, got: {stderr}"
    );

    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["subtree_partition"]["source"], "cargo_workspace");
    let members = parsed["subtree_partition"]["members"]
        .as_array()
        .expect("members array");
    assert_eq!(members.len(), 2);

    let rec = &parsed["records"].as_array().expect("records")[0];
    let rows = rec["subtree_metrics"].as_array().expect("subtree rows");
    assert_eq!(rows.len(), 2, "one row per active subtree");
    assert_eq!(rows[0]["subtree"], "crates/core");
    assert_eq!(rows[0]["edited_file_count"], 0);
    assert_eq!(rows[1]["subtree"], "crates/legacy");
    assert_eq!(rows[1]["edited_file_count"], 1);
    // Repo-wide fields are untouched by the additive schema.
    assert!(rec["retrieval_locality"].is_object());
    assert!(rec["mutation_surface"].is_object());
}

#[test]
fn eval_run_subtree_rows_render_in_human_output() {
    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--repo")
        .arg(fixture("subtree_repo"))
        .assert()
        .success()
        .stdout(predicate::str::contains("crates/core"))
        .stdout(predicate::str::contains("crates/legacy"));
}

// Without --repo there is no partition source: the additive fields are absent
// so existing JSON consumers see an unchanged schema.
#[test]
fn eval_run_omits_subtree_fields_without_repo() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("subtree_partition").is_none());
    let rec = &parsed["records"].as_array().expect("records")[0];
    assert!(rec.get("subtree_metrics").is_none());
}

// --- aoa-2ce: R0b on live data — compose the leakage canary over codeprobe ----

fn r0b_baseline() -> PathBuf {
    fixture("r0b_run_baseline")
}
fn r0b_migrated() -> PathBuf {
    fixture("r0b_run_migrated")
}

// AC1: codeprobe outcomes wire into a run-level RunResult with the correct
// aggregated provenance (External + NativeComposed -> native_composed), and the
// gap is available (not unavailable). A baseline-vs-baseline compare with no
// canary yields a clean label, exercising the wiring end-to-end on a sample run.
#[test]
fn r0b_aggregates_provenance_and_gap_is_available() {
    let output = aoa()
        .args(["eval", "r0b", "--json", "--baseline"])
        .arg(r0b_baseline())
        .arg("--migrated")
        .arg(r0b_baseline()) // self-compare: no movement, no leakage
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");
    assert!(output.status.success(), "clean self-compare exits zero");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["baseline"]["held_out_provenance"], "native_composed");
    assert_eq!(parsed["baseline"]["task_count"], 2);
    // A real (available) gap drives a label rather than a refusal.
    assert_eq!(parsed["status"], "labeled");
    assert!(parsed.get("label").is_some());
}

// AC2: the leakage canary fires on live-shaped data — held-out rises (artifact
// leg flips fail->pass) while visible stays flat (direct leg unchanged) and the
// known held-out canary flips against its declared expectation.
#[test]
fn r0b_leakage_canary_fires_on_live_data() {
    let output = aoa()
        .args(["eval", "r0b", "--json", "--baseline"])
        .arg(r0b_baseline())
        .arg("--migrated")
        .arg(r0b_migrated())
        .arg("--tasks")
        .arg(tasks_dir())
        .arg("--canary")
        .arg(fixture("r0b_canary.json"))
        .output()
        .expect("run");
    assert!(!output.status.success(), "leakage is a gate failure");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["status"], "refused");
    assert_eq!(parsed["kind"], "leakage_detected");
    // The signature: held-out rose, visible flat, canary flipped.
    assert_eq!(parsed["baseline"]["held_out_rate"], 0.5);
    assert_eq!(parsed["migrated"]["held_out_rate"], 1.0);
    assert_eq!(
        parsed["baseline"]["visible_rate"],
        parsed["migrated"]["visible_rate"]
    );
    assert_eq!(parsed["migrated"]["canary_flipped"], true);
}

// AC3: a task family with no independent held-out leg -> gap:unavailable and the
// gate refuses to label (no 'good').
#[test]
fn r0b_no_held_out_leg_is_unavailable_and_refuses_to_label() {
    let run = fixture("r0b_run_unavailable");
    let output = aoa()
        .args(["eval", "r0b", "--json", "--baseline"])
        .arg(&run)
        .arg("--migrated")
        .arg(&run)
        .arg("--tasks")
        .arg(fixture("r0b_tasks_unavailable"))
        .output()
        .expect("run");
    assert!(!output.status.success(), "unavailable gap is a refusal");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["baseline"]["held_out_provenance"], "none");
    assert_eq!(parsed["status"], "refused");
    assert_eq!(parsed["kind"], "gap_unavailable");
    assert!(parsed.get("label").is_none(), "must not emit a label");
}

// A non-dual (single-leg) run has no independent visible leg: R0b fails loud
// naming dual_composite rather than fabricating a visible signal.
#[test]
fn r0b_non_dual_run_fails_loud() {
    let run = fixture("r0b_run_singleleg");
    aoa()
        .args(["eval", "r0b", "--baseline"])
        .arg(&run)
        .arg("--migrated")
        .arg(&run)
        .arg("--tasks")
        .arg(tasks_dir())
        .assert()
        .failure()
        .stderr(predicate::str::contains("dual_composite"));
}

// Human (non-JSON) register renders the leakage refusal as text.
#[test]
fn r0b_human_renders_text() {
    aoa()
        .args(["eval", "r0b", "--baseline"])
        .arg(r0b_baseline())
        .arg("--migrated")
        .arg(r0b_migrated())
        .arg("--tasks")
        .arg(tasks_dir())
        .arg("--canary")
        .arg(fixture("r0b_canary.json"))
        .assert()
        .failure()
        .stdout(predicate::str::contains("aoa eval r0b"))
        .stdout(predicate::str::contains("REFUSED"));
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

// --- aoa-dhk: R0 falsification as a codeprobe experiment ----------------------

// AC2 SMOKE: the full pipeline runs end-to-end on a fixture experiment (1 repo /
// 1 identical-pair task across two arms) and emits falsification.json with a
// verdict field. With a single repo the gate cannot establish a cross-repo
// majority, so the verdict is an honest `inconclusive` carrying the
// `too_few_repos` precondition discriminator — never mistakable for a real
// 5-repo abstention. AC4: codeprobe bias warnings are surfaced alongside.
#[test]
fn experiment_pipeline_smoke_emits_verdict_and_surfaces_bias() {
    let dir = TempDir::new().expect("tempdir");
    let input = dir.path().join("falsify_input.json");
    let build_meta = dir.path().join("falsify_input.build.json");
    let falsification = dir.path().join("falsification.json");

    // Step 1: build the FalsifyInput from the experiment's paired arms.
    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_smoke/manifest.json"))
        .arg("--tasks")
        .arg(fixture("codeprobe_tasks"))
        .arg("--out")
        .arg(&input)
        .assert()
        .success();

    let build: Value =
        serde_json::from_str(&std::fs::read_to_string(&build_meta).expect("build report written"))
            .expect("valid build json");
    assert_eq!(build["repo_count"], 1);
    assert_eq!(build["total_identical_pairs"], 1);
    assert_eq!(build["convention_inputs_degraded"], true);
    let repo0 = &build["repos"][0];
    assert_eq!(repo0["identical_pairs"], 1);
    assert_eq!(
        repo0["eligible"], true,
        "native+high+calibrated repo is eligible"
    );
    // H4: the task present only in the repo arm is excluded as a non-pair.
    let excluded = repo0["excluded_tasks"].as_array().expect("excluded array");
    assert!(
        excluded.iter().any(|e| e["task_id"] == "solo-only-001"),
        "presence-mismatch task must be recorded as excluded, got {excluded:?}"
    );

    // Step 2: run the gate over the built input, with bias warnings attached.
    aoa()
        .args(["falsify", "--repos"])
        .arg(&input)
        .arg("--build-meta")
        .arg(&build_meta)
        .arg("--bias-warnings")
        .arg(fixture("experiment_aggregate.json"))
        .arg("--out")
        .arg(&falsification)
        .assert()
        // A precondition-driven verdict is a non-usable result: non-zero exit.
        .failure();

    let out: Value =
        serde_json::from_str(&std::fs::read_to_string(&falsification).expect("falsification.json"))
            .expect("valid json");
    assert_eq!(out["verdict"], "inconclusive");
    assert_eq!(out["precondition_unmet"], "too_few_repos");
    // AC4: codeprobe bias warnings surfaced alongside the verdict, and the
    // no_independent_baseline warning flagged as gate-invalidating.
    let warnings = out["bias_warnings"]
        .as_array()
        .expect("bias warnings surfaced");
    assert_eq!(warnings.len(), 2);
    assert_eq!(out["bias_gate_invalidating"], true);
}

// H2/AC4: given a real >=5-repo input but a build report flagging degraded
// convention inputs, the gate abstains to `inconclusive` with the
// `convention_inputs_degraded` precondition rather than asserting a verdict the
// R0' convention-invariance check cannot back. The gate's deltas are still
// emitted for transparency.
#[test]
fn falsify_abstains_on_degraded_convention_inputs() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--build-meta")
        .arg(fixture("build_meta_degraded.json"))
        .arg("--bias-warnings")
        .arg(fixture("experiment_aggregate.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .failure();

    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("written")).expect("json");
    assert_eq!(parsed["verdict"], "inconclusive");
    assert_eq!(parsed["precondition_unmet"], "convention_inputs_degraded");
    // Deltas preserved for transparency even when abstaining.
    assert!(
        parsed.get("repo_delta").is_some(),
        "repo_delta kept for transparency"
    );
    assert_eq!(parsed["bias_gate_invalidating"], true);
}

// A genuine >=5-repo gate verdict carries NO precondition discriminator and exits
// zero — the property that keeps a real abstention distinguishable from a
// precondition-driven one.
#[test]
fn falsify_real_verdict_has_no_precondition_marker() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("written")).expect("json");
    assert!(parsed.get("verdict").is_some());
    assert!(
        parsed.get("precondition_unmet").is_none(),
        "a real gate verdict must not carry a precondition discriminator"
    );
}

// Security: untrusted free-text from codeprobe's aggregate.json (bias warning
// messages) must be escaped before reaching the terminal — a crafted message
// must not inject raw control sequences into human output.
#[test]
fn falsify_escapes_untrusted_bias_warning_text() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    let assert = aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--bias-warnings")
        .arg(fixture("bias_warnings_malicious.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    // The raw ESC (0x1b) and BEL (0x07) control bytes must NOT appear in human
    // output; the escaped textual form must.
    assert!(
        !stdout.contains('\u{1b}') && !stdout.contains('\u{07}'),
        "raw control bytes leaked into terminal output"
    );
    assert!(
        stdout.contains("\\u{1b}"),
        "escaped form of the control byte expected"
    );
}

// --- aoa migrate (aoa-mnz.2) ------------------------------------------------

/// A fixture checkout with a manifest-bearing root but no README, so the audit
/// reports a navigability site the migration can fix.
fn migrate_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let p = dir.path();
    std::fs::write(p.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(p.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    dir
}

#[test]
fn migrate_plan_is_dry_run_and_writes_nothing() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("dry-run"))
        .stdout(predicate::str::contains("README.md"));
    assert!(
        !repo.path().join("README.md").exists(),
        "dry-run must not write the anchor"
    );
}

#[test]
fn migrate_apply_then_rollback_round_trips() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        // The human verify line attributes the re-audit to the navigability
        // fix explicitly, so it cannot be read as covering the dead-import
        // fixes the re-audit does not measure.
        .stdout(predicate::str::contains(
            "Re-audit (navigability-anchor) verifies 0 navigability site(s) remaining",
        ));
    assert!(
        repo.path().join("README.md").exists(),
        "apply writes the anchor"
    );

    aoa()
        .args(["migrate", "--rollback", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
    assert!(
        !repo.path().join("README.md").exists(),
        "rollback restores the baseline"
    );
}

#[test]
fn migrate_apply_json_reports_verified_remaining_zero() {
    let repo = migrate_repo();
    let assert = aoa()
        .args(["migrate", "--apply", "--json", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["mode"], "apply");
    // Present (not null) because the navigability fix ran and was re-audited;
    // the count it re-measured is zero.
    assert_eq!(v["navigability_sites_remaining"], 0);
    // Per-fix eligibility: the navigability fix's note is tagged with its id.
    let notes = v["eligibility_notes"]
        .as_array()
        .expect("eligibility_notes");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["fix_id"], "navigability-anchor");
    assert!(notes[0]["note"].as_str().unwrap().contains("code-layer"));
}

#[test]
fn migrate_apply_json_navigability_remaining_is_null_when_nav_fix_excluded() {
    // When the navigability fix is excluded via --fix, its re-audit count is
    // not applicable. The JSON field must serialize as null (not 0, not
    // absent) so a consumer can distinguish "not measured" from "measured
    // zero" — the contract the Option<u64> change introduced.
    let repo = migrate_repo();
    let assert = aoa()
        .args([
            "migrate",
            "--apply",
            "--json",
            "--fix",
            "dead-imports",
            "--repo",
        ])
        .arg(repo.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["mode"], "apply");
    assert!(
        v["navigability_sites_remaining"].is_null(),
        "expected null when the navigability fix did not run, got {:?}",
        v["navigability_sites_remaining"]
    );
    // The navigability anchor must not have been written (fix was excluded).
    assert!(
        !repo.path().join("README.md").exists(),
        "navigability fix was excluded via --fix, so no anchor should be written"
    );
}

#[test]
fn migrate_fix_selector_rejects_unknown_id() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--fix", "no-such-fix", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown fix id"));
}

#[test]
fn migrate_fix_selector_runs_named_fix() {
    let repo = migrate_repo();
    aoa()
        .args([
            "migrate",
            "--fix",
            "navigability-anchor",
            "--apply",
            "--repo",
        ])
        .arg(repo.path())
        .assert()
        .success();
    assert!(
        repo.path().join("README.md").exists(),
        "selected fix ran and wrote the anchor"
    );
}

// aoa-mnz.7: the `aoa gap` subcommand is a live, non-test consumer of
// `current_determination()`. It surfaces the R9c Gating-vs-Advisory
// determination to the operator. With no external-outcome corpus available,
// every gating candidate is Advisory — the surface must say so, naming each
// candidate, rather than silently gating.
#[test]
fn gap_human_surfaces_advisory_determination() {
    aoa()
        .args(["gap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("construct validity"))
        .stdout(predicate::str::contains("Advisory"))
        // a known pre-registered candidate is named
        .stdout(predicate::str::contains("reward_hacking_gap"));
}

#[test]
fn gap_json_carries_every_candidate_as_advisory() {
    let output = aoa().args(["gap", "--json"]).output().expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let metrics = parsed["metrics"].as_array().expect("metrics array");
    assert!(!metrics.is_empty(), "every gating candidate is classified");
    for m in metrics {
        assert_eq!(
            m["mode"], "advisory",
            "no candidate gates without an external-outcome corpus"
        );
    }
    assert!(
        parsed["data_source"].as_str().unwrap().contains("external"),
        "the surface names the data source it consulted"
    );
}

// aoa-d6t.15: the `aoa recommend` subcommand is the connective tissue — it joins
// audit findings + the construct-validity determination + migration availability
// into per-finding recommendations. With no external-outcome corpus, every metric
// is Advisory, so every finding is advisory-only; the surface must say so and
// name the fix availability, never asserting a fix is worth applying.
#[test]
fn recommend_human_surfaces_advisory_findings() {
    // A bare repo is missing Tier-1 planes and has no README -> several findings.
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["recommend", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("AOA recommendations"))
        .stdout(predicate::str::contains("advisory-only"))
        // The footer ties the empty actionable set back to the gap determination.
        .stdout(predicate::str::contains("aoa gap"));
}

#[test]
fn recommend_json_joins_findings_with_metric_and_fix() {
    // A manifest-bearing root without a README yields a navigability finding that
    // HAS a fix (navigability-anchor) but whose metric is Advisory -> the join
    // tags it advisory-only with the metric-advisory reason, fix surfaced.
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();

    let output = aoa()
        .args(["recommend", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success(), "recommend is advisory, exits zero");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    // Counts are present and, with no corpus, nothing is actionable-now.
    assert_eq!(parsed["actionable_now"], 0);
    assert!(parsed["advisory_only"].as_u64().unwrap() >= 1);

    let items = parsed["items"].as_array().expect("items array");
    let nav = items
        .iter()
        .find(|i| i["kind"] == "navigability_anchor")
        .expect("navigability finding present");
    assert_eq!(nav["actionability"], "advisory_only");
    assert_eq!(nav["advisory_reason"], "metric_advisory");
    assert_eq!(nav["metric"], "navigability_anchor_absence");
    assert_eq!(nav["metric_mode"], "advisory");
    assert_eq!(nav["fix_id"], "navigability-anchor");
    assert!(
        nav["fix_eligibility"]
            .as_str()
            .unwrap()
            .contains("code-layer"),
        "the fix's eligibility precondition is surfaced"
    );

    // A missing-plane finding has no gating-candidate metric and no fix: the join
    // distinguishes "no metric" from "metric advisory".
    let plane = items
        .iter()
        .find(|i| i["kind"] == "missing_plane")
        .expect("missing-plane finding present");
    assert!(plane["metric"].is_null(), "plane gap has no metric");
    assert!(plane["fix_id"].is_null(), "no migration for a plane gap");
    assert_eq!(plane["advisory_reason"], "no_fix_available");
}

// R7 keystone end-to-end: the reproduction-before-mutation gate blocks a write
// when no test ran, and allows it once a reproduction span is recorded.
//
// These exercise stdin, so they use `assert_cmd::Command` (which has
// `write_stdin`) rather than the `std::process::Command` helper above.
fn aoa_stdin() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("aoa").expect("aoa binary builds")
}

fn hook_payload(tool: &str, command: Option<&str>, cwd: &Path) -> String {
    let mut input = serde_json::Map::new();
    match command {
        Some(c) => {
            input.insert("command".into(), Value::String(c.into()));
        }
        None => {
            input.insert("file_path".into(), Value::String("src/lib.rs".into()));
        }
    }
    serde_json::to_string(&serde_json::json!({
        "session_id": "it-session",
        "tool_name": tool,
        "tool_input": input,
        "cwd": cwd.to_str().unwrap(),
    }))
    .unwrap()
}

#[test]
fn enforce_check_blocks_write_without_reproduction() {
    let repo = TempDir::new().unwrap();
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked Write"));
}

#[test]
fn enforce_allows_write_after_a_test_run_is_recorded() {
    let repo = TempDir::new().unwrap();

    // 1. Pre-reproduction write is blocked (exit 2).
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2);

    // 2. A test command records a test.run span (PostToolUse, never blocks).
    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("cargo test --all"), repo.path()))
        .assert()
        .success();

    // 3. The same write now passes the gate (exit 0).
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .success();

    // The live log carries the test.run and the earlier write.blocked span.
    let log = repo.path().join(".aoa/traces/live-it-session.jsonl");
    let contents = std::fs::read_to_string(&log).expect("live log written");
    assert!(contents.contains("test.run"));
    assert!(contents.contains("write.blocked"));
}

#[test]
fn enforce_check_blocks_protected_path_even_without_reproduction() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\".github/**\"]\nreproduction_required: false\n",
    )
    .unwrap();

    let mut input = serde_json::Map::new();
    input.insert(
        "file_path".into(),
        Value::String(".github/workflows/ci.yml".into()),
    );
    let payload = serde_json::to_string(&serde_json::json!({
        "session_id": "it-prot",
        "tool_name": "Write",
        "tool_input": input,
        "cwd": repo.path().to_str().unwrap(),
    }))
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(payload)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("protected path"));
}

#[test]
fn enforce_reproduction_toggle_off_allows_unprotected_write() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\".github/**\"]\nreproduction_required: false\n",
    )
    .unwrap();
    // Unprotected path, gate disabled by policy -> allowed with no test.run.
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .success();
}

/// A hook payload writing to an explicit `file_path` (the generated/protected
/// path tests need a target other than the default `src/lib.rs`).
fn write_payload(file_path: &str, session: &str, cwd: &Path) -> String {
    let mut input = serde_json::Map::new();
    input.insert("file_path".into(), Value::String(file_path.into()));
    serde_json::to_string(&serde_json::json!({
        "session_id": session,
        "tool_name": "Write",
        "tool_input": input,
        "cwd": cwd.to_str().unwrap(),
    }))
    .unwrap()
}

#[test]
fn enforce_check_blocks_write_to_declared_generated_path() {
    let repo = TempDir::new().unwrap();
    // Gate off so only the R6 generated-artifact block can fire — isolates it.
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n\
         generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n",
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_payload(
            "crates/api/types.gen.rs",
            "it-gen",
            repo.path(),
        ))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("generated artifact"))
        .stderr(predicate::str::contains("schema.json"));

    // The write.blocked span records the source as its own machine-readable attr.
    let log = repo.path().join(".aoa/traces/live-it-gen.jsonl");
    let contents = std::fs::read_to_string(&log).expect("live log written");
    assert!(contents.contains("write.blocked"));
    assert!(contents.contains("generated_artifact"));
    assert!(contents.contains("\"source\":\"schema.json\""));
}

#[test]
fn enforce_check_blocks_write_to_bare_glob_generated_path() {
    // Back-compat form: a bare-string generated_paths entry (no `source:`). The
    // block must still fire; the redirect falls back to the glob itself.
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n\
         generated_paths:\n  - \"**/*.gen.rs\"\n",
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_payload(
            "crates/api/types.gen.rs",
            "it-bare",
            repo.path(),
        ))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("generated artifact"))
        // No declared source -> the redirect names the glob itself.
        .stderr(predicate::str::contains("**/*.gen.rs"));

    let log = repo.path().join(".aoa/traces/live-it-bare.jsonl");
    let contents = std::fs::read_to_string(&log).expect("live log written");
    assert!(contents.contains("write.blocked"));
    assert!(contents.contains("\"source\":\"**/*.gen.rs\""));
}

#[test]
fn enforce_check_allows_write_to_non_generated_path() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n\
         generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n",
    )
    .unwrap();
    // A hand-written source file is not generated -> allowed.
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_payload(
            "crates/api/handler.rs",
            "it-gen-ok",
            repo.path(),
        ))
        .assert()
        .success();
}

#[test]
fn policy_compile_writes_three_planes_idempotently() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\ngateway_allowlist: [\"src/db/gateway.rs\"]\n",
    )
    .unwrap();

    let compile = || {
        aoa_stdin()
            .args(["policy", "compile", "--repo", repo.path().to_str().unwrap()])
            .assert()
            .success();
    };
    compile();

    let settings = repo.path().join(".claude/settings.json");
    let precommit = repo.path().join(".pre-commit-config.yaml");
    let workflow = repo.path().join(".github/workflows/aoa-policy.yml");
    let owners = repo.path().join(".github/CODEOWNERS");
    for p in [&settings, &precommit, &workflow, &owners] {
        assert!(p.exists(), "missing plane artifact: {}", p.display());
    }
    let snapshot: Vec<String> = [&settings, &precommit, &workflow, &owners]
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();

    // Re-compile: every artifact is byte-stable.
    compile();
    for (p, before) in [&settings, &precommit, &workflow, &owners]
        .iter()
        .zip(&snapshot)
    {
        assert_eq!(
            &std::fs::read_to_string(p).unwrap(),
            before,
            "{} not idempotent",
            p.display()
        );
    }
    // CI workflow embeds the protected glob; CODEOWNERS lists the gateway.
    assert!(snapshot[2].contains("'migrations/**'"));
    assert!(snapshot[3].contains("src/db/gateway.rs @owners"));
}

#[test]
fn policy_compile_emits_gitattributes_marking_for_generated_paths() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n",
    )
    .unwrap();
    // A pre-existing user entry must survive the compile (non-destructive merge).
    let gitattributes = repo.path().join(".gitattributes");
    std::fs::write(&gitattributes, "* text=auto\n").unwrap();

    let compile = || {
        aoa_stdin()
            .args(["policy", "compile", "--repo", repo.path().to_str().unwrap()])
            .assert()
            .success();
    };
    compile();

    let attrs = std::fs::read_to_string(&gitattributes).unwrap();
    // User content preserved + the R6 entry and provenance header emitted.
    assert!(attrs.contains("* text=auto"), "user line preserved");
    assert!(attrs.contains("**/*.gen.rs linguist-generated -diff"));
    assert!(attrs.contains("@generated"));
    assert!(attrs.contains("schema.json"));

    // Idempotent: a second compile rewrites byte-identical content.
    compile();
    assert_eq!(std::fs::read_to_string(&gitattributes).unwrap(), attrs);
}

#[test]
fn policy_guard_staged_rejects_protected_file() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\n",
    )
    .unwrap();

    // A protected staged file fails the pre-commit guard.
    aoa_stdin()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .arg("migrations/0001.sql")
        .assert()
        .failure()
        .stderr(predicate::str::contains("protected path"));

    // An ordinary file passes.
    aoa_stdin()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .arg("src/lib.rs")
        .assert()
        .success();
}

#[test]
fn enforce_ignores_non_mutation_tools() {
    let repo = TempDir::new().unwrap();
    // A Read is not a guarded mutation: allowed even with no reproduction.
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Read", None, repo.path()))
        .assert()
        .success();
}

#[test]
fn observe_enforce_writes_idempotent_settings_and_plain_observe_does_not() {
    let repo = TempDir::new().unwrap();
    let settings = repo.path().join(".claude/settings.json");

    aoa_stdin()
        .args([
            "observe",
            "--repo",
            repo.path().to_str().unwrap(),
            "--enforce",
        ])
        .assert()
        .success();
    let first = std::fs::read_to_string(&settings).expect("settings written");
    assert!(first.contains("aoa enforce check"));
    assert!(first.contains("aoa enforce record"));

    // Re-running is byte-stable (idempotent merge).
    aoa_stdin()
        .args([
            "observe",
            "--repo",
            repo.path().to_str().unwrap(),
            "--enforce",
        ])
        .assert()
        .success();
    let second = std::fs::read_to_string(&settings).unwrap();
    assert_eq!(first, second, "second observe --enforce must be a no-op");

    // Plain observe (no --enforce) installs no hook.
    let plain = TempDir::new().unwrap();
    aoa_stdin()
        .args(["observe", "--repo", plain.path().to_str().unwrap()])
        .assert()
        .success();
    assert!(!plain.path().join(".claude/settings.json").exists());
}

// --- aoa-hal.6: R16 ownership inference + R17 dual-register uniformity --------

/// A two-author git repo: alice owns `a/`, bob owns `b/` and the root file.
fn blame_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/one.txt"), "line\nline\nline\n").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(
        dir.path(),
        &[
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@example.com",
            "commit",
            "-qm",
            "alice adds a/",
        ],
    );
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    std::fs::write(dir.path().join("b/two.txt"), "x\n").unwrap();
    std::fs::write(dir.path().join("ROOT.md"), "root\n").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(
        dir.path(),
        &[
            "-c",
            "user.name=Bob",
            "-c",
            "user.email=bob@example.com",
            "commit",
            "-qm",
            "bob adds b/ and root",
        ],
    );
    dir
}

// R16 AC: infer-owners emits a reviewable CODEOWNERS diff and never writes
// without --write.
#[test]
fn infer_owners_prints_reviewable_diff_without_writing() {
    let repo = blame_repo();
    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("create: .github/CODEOWNERS"))
        .stdout(predicate::str::contains("+/a/ alice@example.com"))
        .stdout(predicate::str::contains("+/b/ bob@example.com"))
        .stdout(predicate::str::contains("+/* bob@example.com"));
    assert!(
        !repo.path().join(".github/CODEOWNERS").exists(),
        "default run must not write CODEOWNERS"
    );
}

#[test]
fn infer_owners_write_writes_the_proposal() {
    let repo = blame_repo();
    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--write",
        ])
        .assert()
        .success();
    let owners = std::fs::read_to_string(repo.path().join(".github/CODEOWNERS")).unwrap();
    assert!(owners.starts_with("# PROPOSED by `aoa policy infer-owners`"));
    assert!(owners.contains("/a/ alice@example.com\n"));
    assert!(owners.contains("/b/ bob@example.com\n"));
}

// R17 AC: the JSON register carries the same findings as the human diff.
#[test]
fn infer_owners_json_matches_human_findings() {
    let repo = blame_repo();
    let output = aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["written"], false);
    let entries = parsed["entries"].as_array().expect("entries array");
    let alice = entries
        .iter()
        .find(|e| e["pattern"] == "/a/")
        .expect("/a/ entry");
    assert_eq!(alice["owner"], "alice@example.com");
    assert_eq!(alice["owned_lines"], 3);
    assert_eq!(alice["total_lines"], 3);
    // Identical findings across registers: every JSON entry appears in the diff.
    let diff = parsed["diff"].as_str().expect("diff string");
    for entry in entries {
        let line = format!(
            "+{} {}",
            entry["pattern"].as_str().unwrap(),
            entry["owner"].as_str().unwrap()
        );
        assert!(diff.contains(&line), "JSON entry missing from diff: {line}");
    }
}

// R16: enumeration and attribution share HEAD as the source of truth — a
// staged-but-uncommitted file (a routine mid-task index state) must not abort
// the command with a failed `git blame HEAD` on a path HEAD has never seen.
#[test]
fn infer_owners_ignores_staged_but_uncommitted_files() {
    let repo = blame_repo();
    std::fs::create_dir_all(repo.path().join("staged")).unwrap();
    std::fs::write(repo.path().join("staged/new.txt"), "uncommitted\n").unwrap();
    run_git(repo.path(), &["add", "staged/new.txt"]);
    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("+/a/ alice@example.com"))
        .stdout(predicate::str::contains("/staged/").not());
}

// R16: a merge-conflicted file lists one index entry per stage (1/2/3); its
// lines must be counted once, not once per stage, or its authors gain 3x
// weight and the reported arithmetic is silently wrong.
#[test]
fn infer_owners_counts_conflicted_files_once() {
    let repo = TempDir::new().unwrap();
    init_git_repo(repo.path());
    std::fs::create_dir_all(repo.path().join("c")).unwrap();
    std::fs::write(repo.path().join("c/f.txt"), "one\ntwo\n").unwrap();
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "-qm", "base"]);
    run_git(repo.path(), &["checkout", "-qb", "side"]);
    std::fs::write(repo.path().join("c/f.txt"), "one\ntwo side\n").unwrap();
    run_git(repo.path(), &["commit", "-aqm", "side edit"]);
    run_git(repo.path(), &["checkout", "-q", "-"]);
    std::fs::write(repo.path().join("c/f.txt"), "one\ntwo main\n").unwrap();
    run_git(repo.path(), &["commit", "-aqm", "main edit"]);
    // The merge conflicts by construction; leave the index unmerged.
    let merge = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["merge", "-q", "side"])
        .output()
        .expect("git available");
    assert!(!merge.status.success(), "merge must conflict");

    let output = aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let entries = parsed["entries"].as_array().expect("entries array");
    let c = entries
        .iter()
        .find(|e| e["pattern"] == "/c/")
        .expect("/c/ entry");
    assert_eq!(
        c["total_lines"], 2,
        "2-line file must not be triple-counted"
    );
    assert_eq!(c["owned_lines"], 2);
}

// R17: with zero attributed entries both registers carry nothing actionable —
// the JSON must not advertise a create-diff that the human register omits and
// that --write refuses.
#[test]
fn infer_owners_zero_entries_keeps_registers_in_parity() {
    let repo = TempDir::new().unwrap();
    init_git_repo(repo.path());
    run_git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "empty"],
    );

    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to propose"));

    let output = aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["entries"], serde_json::json!([]));
    assert_eq!(
        parsed["proposal"], "",
        "no proposal content without entries"
    );
    assert_eq!(parsed["diff"], "", "no diff the human register never shows");
}

// R17: policy compile exposes the JSON register listing the written planes.
#[test]
fn policy_compile_json_lists_written_planes() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\n",
    )
    .unwrap();
    let output = aoa()
        .args([
            "policy",
            "compile",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let planes: Vec<&str> = parsed["planes_written"]
        .as_array()
        .expect("planes_written array")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(planes
        .iter()
        .any(|p| p.ends_with(".pre-commit-config.yaml")));
    assert!(planes.iter().any(|p| p.ends_with("CODEOWNERS")));
}

// R17: guard-staged exposes the JSON register carrying the blocked findings and
// keeps the failure exit code.
#[test]
fn policy_guard_staged_json_carries_blocked_findings() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\n",
    )
    .unwrap();

    let output = aoa()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
            "migrations/0001.sql",
            "src/lib.rs",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(
        parsed["blocked"],
        serde_json::json!(["migrations/0001.sql"])
    );

    let clean = aoa()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
            "src/lib.rs",
        ])
        .output()
        .expect("run");
    assert!(clean.status.success());
    let parsed: Value = serde_json::from_slice(&clean.stdout).expect("valid json");
    assert_eq!(parsed["blocked"], serde_json::json!([]));
}

// R17: observe exposes the JSON register reporting the installed paths.
#[test]
fn observe_json_reports_installed_paths() {
    let repo = TempDir::new().unwrap();
    let output = aoa()
        .args(["observe", "--repo", repo.path().to_str().unwrap(), "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed["traces_dir"].as_str().unwrap().ends_with("traces"));
    assert!(parsed["gitignore"].is_string());
    assert!(parsed["enforce_settings"].is_null());
}
