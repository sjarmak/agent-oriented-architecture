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
