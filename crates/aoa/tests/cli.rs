use std::path::{Path, PathBuf};
use std::process::Command;

use aoa_gap::MIN_HELD_OUT_OBSERVATIONS;
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

/// Both post-parse failures used to reach the CLI path-free and were named only
/// by an anyhow context here. Now `aoa-trace` names the file at its own boundary,
/// so assert the rendered diagnostic identifies the offending file and that
/// neither the filename nor the reason is printed twice by the stacked context
/// and source-chain rendering.
///
/// Driven over both variants because the acceptance criteria pairs them: a
/// future variant-specific context added back in `eval.rs` must fail here.
fn assert_trace_error_names_file_once(fixture_name: &str, reason: &str) {
    let output = aoa()
        .args(["eval", "validate-trace"])
        .arg(fixture(fixture_name))
        .output()
        .expect("run");
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches(fixture_name).count(),
        1,
        "diagnostic must name the offending file exactly once: {stderr}"
    );
    assert_eq!(
        stderr.matches(reason).count(),
        1,
        "diagnostic must state the reason exactly once: {stderr}"
    );
}

#[test]
fn validate_trace_ordering_error_names_the_file_once() {
    assert_trace_error_names_file_once("invalid_trace.json", "out of order");
}

#[test]
fn validate_trace_version_error_names_the_file_once() {
    assert_trace_error_names_file_once("bad_version_trace.json", "unsupported wire-format version");
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

// aoa-vme7: a scoring.json carrying NEITHER `score` nor `passed` is a trial with
// no held-out signal at all. It must be excluded and reported, not scored as a
// genuine failure — otherwise a malformed trial silently biases the run's
// behavioral signal toward pessimism. The trial gets a REAL transcript so the
// failure can only come from the scoring decode: with a missing agent_output.txt
// the trace-shim would error first and this test would pass without ever
// reaching the code it pins.
#[test]
fn eval_run_excludes_a_trial_whose_scoring_carries_no_signal() {
    // Both trials are copies of real fixture trials — same transcripts, same
    // task ids so both oracles load. The ONLY difference is the no-signal
    // trial's scoring.json, so nothing else can be what fails.
    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("run");
    for id in ["native-consensus-001", "external-filelist-000"] {
        let trial = run.join(id);
        std::fs::create_dir_all(&trial).expect("trial dir");
        std::fs::copy(
            run_dir().join(id).join("agent_output.txt"),
            trial.join("agent_output.txt"),
        )
        .expect("copy transcript");
        std::fs::copy(
            run_dir().join(id).join("scoring.json"),
            trial.join("scoring.json"),
        )
        .expect("copy scoring");
    }
    let no_signal = run.join("external-filelist-000");
    // Well-formed JSON, plausible codeprobe shape — just no outcome field.
    std::fs::write(
        no_signal.join("scoring.json"),
        br#"{"reward":0.0,"status":"completed"}"#,
    )
    .expect("write scoring");

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");

    assert!(
        !output.status.success(),
        "a trial with no held-out signal must fail loud"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["record_count"], 1, "only the good trial records");
    assert_eq!(parsed["error_count"], 1);
    // The harm this pins: the no-signal trial must not reach the run's
    // behavioral signal as an observation of a real held-out failure.
    assert_eq!(parsed["behavioral_signal"]["observations"], 1);
    assert!(
        parsed["records"]
            .as_array()
            .expect("records array")
            .iter()
            .all(|r| r["task_id"] == "native-consensus-001"),
        "the no-signal trial must not produce a record"
    );

    let errors = parsed["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["task_id"], "external-filelist-000");
    let message = errors[0]["error"].as_str().expect("error string");
    for expected in ["scoring.json", "passed", "score"] {
        assert!(
            message.contains(expected),
            "error must name {expected}: {message}"
        );
    }
}

// A trial dir whose NAME is not valid UTF-8 aborts the command instead of being
// silently skipped. Discovery sits upstream of this command's per-trial
// isolation, so the whole batch fails — pinned here so a future refactor cannot
// return it to a skip. See aoa-m8rb for whether it should isolate instead.
#[cfg(unix)]
#[test]
fn eval_run_aborts_on_a_non_utf8_trial_dir_name() {
    use std::os::unix::ffi::OsStringExt;

    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("run");
    std::fs::create_dir_all(&run).expect("run dir");
    // One real trial copied from the fixture, so the run dir is otherwise valid
    // and a skip would look like success with one fewer record.
    let good = run.join("native-consensus-001");
    std::fs::create_dir_all(&good).expect("good trial");
    for artifact in ["scoring.json", "agent_output.txt"] {
        std::fs::copy(
            run_dir().join("native-consensus-001").join(artifact),
            good.join(artifact),
        )
        .expect("copy artifact");
    }
    let bad = run.join(std::ffi::OsString::from_vec(b"task-\xffbad".to_vec()));
    std::fs::create_dir_all(&bad).expect("bad trial");
    std::fs::copy(good.join("scoring.json"), bad.join("scoring.json")).expect("copy scoring");

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");

    assert!(
        !output.status.success(),
        "a non-UTF-8 trial dir name must not be silently skipped"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not valid UTF-8"),
        "error must name the cause: {stderr}"
    );
    // The offending name reaches stderr escaped, never as raw bytes.
    assert!(
        !output.stderr.contains(&0xff),
        "raw invalid byte survived into stderr: {stderr}"
    );
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
    // Per-subtree mutation surface vs cross-subtree leakage (aoa-d6t.30): the
    // fixture's core crate calls into the legacy crate, so from core one
    // writable node is reachable inside (its own def) and one across the
    // boundary; legacy reaches only itself.
    assert_eq!(rows[0]["mutation_surface"], 1);
    assert_eq!(rows[0]["mutation_leakage"], 1);
    assert_eq!(rows[1]["mutation_surface"], 1);
    assert_eq!(rows[1]["mutation_leakage"], 0);
    // Both rows seeded nodes, so no unavailability marker is emitted.
    assert!(rows[0].get("mutation_unavailable").is_none());
    assert!(rows[1].get("mutation_unavailable").is_none());
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

// --- aoa-d6t.32: explicit --subtree-root partition source ----------------------

// --scip-index conflicts with --repo, so a SCIP-graded run has no automatic
// partition source: --subtree-root supplies one, and the graph stays
// SCIP-quality.
#[test]
fn eval_run_scip_index_takes_subtree_root_partition() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--scip-index")
        .arg(fixture("subtree_scip_index.json"))
        .arg("--subtree-root")
        .arg(fixture("subtree_repo"))
        .output()
        .expect("run");
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("per-subtree"),
        "per-subtree mode must be logged, got: {stderr}"
    );

    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["subtree_partition"]["source"], "cargo_workspace");
    let rec = &parsed["records"].as_array().expect("records")[0];
    assert_eq!(rec["graph_quality"], "scip", "graph stays SCIP-graded");
    let rows = rec["subtree_metrics"].as_array().expect("subtree rows");
    assert_eq!(rows.len(), 2, "one row per active subtree");
    assert_eq!(rows[0]["subtree"], "crates/core");
    assert_eq!(rows[1]["subtree"], "crates/legacy");
}

// An explicit --subtree-root naming a nonexistent directory must not be
// silently ignored: the run still completes repo-wide, but the dropped flag is
// surfaced on stderr ("surface and fall back, never guess").
#[test]
fn eval_run_warns_when_subtree_root_is_not_a_directory() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--scip-index")
        .arg(fixture("subtree_scip_index.json"))
        .args(["--subtree-root", "/does/not/exist"])
        .output()
        .expect("run");
    assert!(output.status.success(), "falls back, does not abort");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--subtree-root") && stderr.contains("not a directory"),
        "dropped explicit flag must be surfaced, got: {stderr}"
    );

    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("subtree_partition").is_none());
}

// An explicit --subtree-root naming a real directory that has no multi-member
// workspace manifest is likewise surfaced, so "wrong path" is never read as
// "this repo has no workspace".
#[test]
fn eval_run_warns_when_subtree_root_has_no_workspace() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--scip-index")
        .arg(fixture("subtree_scip_index.json"))
        .arg("--subtree-root")
        .arg(fixture("codeprobe_tasks"))
        .output()
        .expect("run");
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--subtree-root") && stderr.contains("no multi-member workspace"),
        "unpartitioned explicit root must be surfaced, got: {stderr}"
    );

    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("subtree_partition").is_none());
}

// The automatic --repo path keeps its silence for an unpartitioned checkout:
// no flag was dropped, so there is nothing to surface.
#[test]
fn eval_run_stays_silent_for_unpartitioned_repo_without_subtree_root() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--repo")
        .arg(fixture("codeprobe_tasks"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("--subtree-root"),
        "automatic path must not warn, got: {stderr}"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("subtree_partition").is_none());
}

// In --repo mode an explicit --subtree-root wins as the partition source (the
// graph still comes from --repo): here --repo is not a workspace, so any
// partition present came from --subtree-root.
#[test]
fn eval_run_subtree_root_overrides_repo_partition_source() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(fixture("subtree_run"))
        .arg("--repo")
        .arg(fixture("codeprobe_tasks"))
        .arg("--subtree-root")
        .arg(fixture("subtree_repo"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["subtree_partition"]["source"], "cargo_workspace");
    let rec = &parsed["records"].as_array().expect("records")[0];
    let rows = rec["subtree_metrics"].as_array().expect("subtree rows");
    assert_eq!(rows.len(), 2, "partition sourced from --subtree-root");
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

// Answer-task conventions (pre-registered 2026-07-04, aoa-dhk.1): an
// answer-shaped repo (task_shape "answer" + scip_index) gets REAL per-pair
// trace-locality/trace-reach inputs joined from both arms' trial traces, the
// task oracle chain, and the SCIP graph. The pair whose trials carry no
// instrumented file access is excluded with the reason; the surviving pair's
// inputs are exact; convention_inputs_degraded is an honest computed false and
// the config carries the answer-family convention set.
#[test]
fn experiment_answer_shape_computes_real_convention_inputs() {
    let dir = TempDir::new().expect("tempdir");
    let input = dir.path().join("falsify_input.json");
    let build_meta = dir.path().join("falsify_input.build.json");
    let falsification = dir.path().join("falsification.json");

    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_answer/manifest.json"))
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(&input)
        .assert()
        .success();

    let build: Value =
        serde_json::from_str(&std::fs::read_to_string(&build_meta).expect("build report written"))
            .expect("valid build json");
    assert_eq!(build["task_shape"], "answer");
    assert_eq!(
        build["convention_inputs_degraded"], false,
        "every admitted pair carries real inputs, so the flag is computed false"
    );
    assert_eq!(build["repos"][0]["identical_pairs"], 1);
    let excluded = build["repos"][0]["excluded_tasks"]
        .as_array()
        .expect("excluded array");
    let noreads = excluded
        .iter()
        .find(|e| e["task_id"] == "comprehension-noreads-001")
        .expect("prose-only-transcript pair is excluded");
    assert!(
        noreads["reason"]
            .as_str()
            .unwrap()
            .contains("trace footprint is empty"),
        "exclusion carries the computed reason, got {noreads:?}"
    );

    // The built input carries the exact per-arm inputs: repo arm read exactly
    // the two oracle-chain files (locality 1.0, depth 0); harness arm read one
    // chain file and one off-chain file (locality 0.5) whose reach to the
    // remaining chain file is one undirected hop (depth 1).
    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("input written"))
            .expect("valid input json");
    let task = &parsed["repos"][0]["runs"][0]["tasks"][0];
    let inputs = &task["convention_inputs"];
    assert_eq!(inputs["family"], "answer");
    assert_eq!(inputs["repo_trace_locality"], 1.0);
    assert_eq!(inputs["harness_trace_locality"], 0.5);
    assert_eq!(inputs["repo_trace_reach_depth"], 0);
    assert_eq!(inputs["harness_trace_reach_depth"], 1);
    let conventions = parsed["config"]["conventions"]
        .as_array()
        .expect("conventions");
    assert!(conventions
        .iter()
        .any(|c| c["name"] == "trace_locality_floor" && c["family"] == "answer"));

    // The gate still abstains on too_few_repos (1 repo), but WITHOUT any
    // convention-degradation marker: that blocker is genuinely cleared.
    aoa()
        .args(["falsify", "--repos"])
        .arg(&input)
        .arg("--build-meta")
        .arg(&build_meta)
        .arg("--out")
        .arg(&falsification)
        .assert()
        .failure();
    let out: Value =
        serde_json::from_str(&std::fs::read_to_string(&falsification).expect("falsification"))
            .expect("valid json");
    assert_eq!(out["precondition_unmet"], "too_few_repos");
    let notes = out["notes"].as_array().unwrap();
    assert!(
        !notes
            .iter()
            .any(|n| n.as_str().unwrap_or_default().contains("degraded")),
        "no degraded-convention note may appear for an answer-shape build, got {notes:?}"
    );
}

/// Writes `manifest_json` to a temp dir and runs the experiment builder over
/// it against the `answer_tasks` fixture, expecting failure; callers assert
/// on stderr.
fn experiment_manifest_failure(manifest_json: &str) -> assert_cmd::assert::Assert {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("manifest.json");
    std::fs::write(&manifest, manifest_json).expect("manifest written");
    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(&manifest)
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(dir.path().join("falsify_input.json"))
        .assert()
        .failure()
}

// A manifest declaring answer shape without the index it needs fails loud —
// the builder never silently degrades an operator-declared answer repo.
#[test]
fn experiment_answer_shape_without_index_fails_loud() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/answers", "confidence": "high", "calibrated": true,
            "task_shape": "answer",
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" },
              { "seed": 2, "repo_arm": "seed2/repo_arm", "harness_arm": "seed2/harness_arm" },
              { "seed": 3, "repo_arm": "seed3/repo_arm", "harness_arm": "seed3/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains("requires scip_index"));
}

// `deny_unknown_fields` is per-struct, so each of the three manifest
// boundaries (Manifest, RepoManifest, RunManifest) is exercised separately;
// the parse fails at the unknown key, so one run entry suffices regardless
// of `k_runs`. Rationale for the strictness lives on the Manifest doc.
#[test]
fn experiment_manifest_rejects_unknown_fields_at_every_boundary() {
    let repo = r#""repo_id": "sample/repo", "confidence": "high", "calibrated": true"#;
    let run = r#""seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm""#;
    let cases = [
        (
            "min_effect_szie",
            format!(
                r#"{{ "k_runs": 3, "min_holdout_size": 1, "min_effect_szie": 0.05,
                     "repos": [{{ {repo}, "runs": [{{ {run} }}] }}] }}"#
            ),
        ),
        (
            "task_shpae",
            format!(
                r#"{{ "k_runs": 3, "min_holdout_size": 1,
                     "repos": [{{ {repo}, "task_shpae": "answer", "runs": [{{ {run} }}] }}] }}"#
            ),
        ),
        (
            "sede",
            format!(
                r#"{{ "k_runs": 3, "min_holdout_size": 1,
                     "repos": [{{ {repo}, "runs": [{{ "sede": 1, {run} }}] }}] }}"#
            ),
        ),
    ];

    for (typo, manifest_json) in cases {
        experiment_manifest_failure(&manifest_json)
            .stderr(predicate::str::contains("unknown field"))
            .stderr(predicate::str::contains(typo));
    }
}

// The strictness above must not reject the documented R11 attestation key
// (docs/r0_runbook.md § "R11 scope note" tells answer-shape operators to
// declare `calibrated_basis`). Reaching the post-parse `requires scip_index`
// diagnostic proves the manifest deserialized cleanly with the key present.
#[test]
fn experiment_manifest_accepts_documented_calibrated_basis_key() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/answers", "confidence": "high", "calibrated": true,
            "calibrated_basis": "consensus-verified-answer-oracles-r11-scope-note-2026-07-05",
            "task_shape": "answer",
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains("requires scip_index"))
    .stderr(predicate::str::contains("unknown field").not());
}

// aoa-g2g5: R0 determinism evidence is only valid across INDEPENDENT,
// identity-aligned runs. The builder's old run-count/min-max-count checks let
// three integrity violations through; each must now fail loud.

// A repo whose runs reuse a seed is K copies of one draw, not K independent
// replications — reject it before any evidence is assembled. (Bails pre-read,
// so the run dirs need not exist.)
#[test]
fn experiment_rejects_duplicate_seed_across_runs() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/dup-seed", "confidence": "high", "calibrated": true,
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" },
              { "seed": 1, "repo_arm": "seed2/repo_arm", "harness_arm": "seed2/harness_arm" },
              { "seed": 3, "repo_arm": "seed3/repo_arm", "harness_arm": "seed3/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains(
        "seed 1 is used by more than one run",
    ));
}

// Reusing an arm run directory across runs reads that arm's outcomes from
// identical files — again not an independent replication. (Distinct seeds here,
// so the seed gate passes and the directory gate is what fires.)
#[test]
fn experiment_rejects_reused_run_directory() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/dup-dir", "confidence": "high", "calibrated": true,
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" },
              { "seed": 2, "repo_arm": "seed1/repo_arm", "harness_arm": "seed2/harness_arm" },
              { "seed": 3, "repo_arm": "seed3/repo_arm", "harness_arm": "seed3/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains("run directory"))
    .stderr(predicate::str::contains("used by more than one run/arm"));
}

// Directory distinctness resolves ./.. (and symlinks): two runs naming the SAME
// physical dir via different spellings must still be rejected, or distinct seeds
// could read identical outcomes (vacuous replication). Uses absolute paths into
// an existing fixture so canonicalize() actually resolves the `..` alias.
#[test]
fn experiment_rejects_aliased_run_directory() {
    let smoke = fixture("experiment_smoke");
    let smoke = smoke.to_str().unwrap();
    let manifest = format!(
        r#"{{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{{
            "repo_id": "sample/alias", "confidence": "high", "calibrated": true,
            "runs": [
              {{ "seed": 1, "repo_arm": "{smoke}/seed1/repo_arm", "harness_arm": "{smoke}/seed1/harness_arm" }},
              {{ "seed": 2, "repo_arm": "{smoke}/seed2/../seed1/repo_arm", "harness_arm": "{smoke}/seed2/harness_arm" }},
              {{ "seed": 3, "repo_arm": "{smoke}/seed3/repo_arm", "harness_arm": "{smoke}/seed3/harness_arm" }}
            ]
          }}]
        }}"#
    );
    experiment_manifest_failure(&manifest)
        .stderr(predicate::str::contains("used by more than one run/arm"));
}

// The core aoa-g2g5 case: two runs admit the SAME NUMBER of identical pairs but
// DIFFERENT task identities (run 0/2 admit `alpha`, run 1 admits `beta`). The
// old min/max-count check passed this (counts all equal 1); the identity check
// must fail it, naming the missing/extra ids, because the positional PairTask
// ids and run-indexed determinism check would otherwise compare mismatched
// tasks.
#[test]
fn experiment_rejects_equal_count_different_task_identities() {
    let dir = TempDir::new().expect("tempdir");
    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_divergent/manifest.json"))
        .arg("--tasks")
        .arg(fixture("experiment_divergent"))
        .arg("--out")
        .arg(dir.path().join("falsify_input.json"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "admits a different identical-pair set than run 0",
        ))
        .stderr(predicate::str::contains("\"alpha\""))
        .stderr(predicate::str::contains("\"beta\""));
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
        .arg("--build-meta")
        .arg(fixture("build_meta_ok.json"))
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
        .arg("--build-meta")
        .arg(fixture("build_meta_ok.json"))
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

// --- aoa-d6t.23: greenfield/cold-start InsufficientData across the surfaces ---

const INSUFFICIENT_REASON: &str = "no held-out behavioral signal for this repo yet";

/// Accumulate `n` observe-captured live sessions under `<repo>/.aoa/traces/`,
/// each carrying a landed edit — a session counts as a held-out behavioral
/// observation only when it holds a real edit out.
///
/// Each session records the full write lifecycle the hooks emit: the
/// `write.attempt` logged before the tool runs, then the `write.committed`
/// logged once it succeeds. The attempt alone would not do, because an attempt
/// is not a landed edit.
fn seed_live_sessions(repo: &Path, n: usize) {
    seed_live_sessions_with_spans(
        repo,
        n,
        concat!(
            r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#,
            "\n",
            r#"{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"src/app.py"}}"#,
            "\n",
            r#"{"type":"write.committed","source":"native","seq":2,"attributes":{"path":"src/app.py"}}"#,
            "\n",
        ),
    );
}

fn seed_live_sessions_with_spans(repo: &Path, n: usize, spans: &str) {
    let traces = repo.join(".aoa").join("traces");
    std::fs::create_dir_all(&traces).expect("create traces dir");
    for i in 0..n {
        std::fs::write(traces.join(format!("live-s{i}.jsonl")), spans).expect("write live log");
    }
}

// Sessions captured before the outcome hooks existed hold attempts and nothing
// else. None of them proves an edit landed, so they must not be counted as
// held-out observations — and the shortfall has to surface as the explicit
// InsufficientData reason rather than as a confident score over zero evidence.
// This is the upgrade path: a binary with the new hooks reading a repo whose
// `.claude/settings.json` still only registers the old ones sees exactly this.
#[test]
fn attempt_only_sessions_are_not_counted_as_landed_edits() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions_with_spans(
        repo.path(),
        10,
        concat!(
            r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#,
            "\n",
            r#"{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"src/app.py"}}"#,
            "\n",
        ),
    );

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    assert_eq!(
        parsed["behavioral_signal"]["observations"], 0,
        "an attempt with no committed outcome is not a landed edit"
    );
    assert_eq!(
        parsed["insufficient_data"]["reason"], INSUFFICIENT_REASON,
        "the shortfall must be stated, not silently scored as zero"
    );
    assert!(
        !parsed["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|i| i["kind"] == "mutation_surface"),
        "no behavioral score may be fabricated from attempt-only sessions"
    );
}

// A repo with no observe-captured held-out signal: audit reports
// InsufficientData with the reason, and no fabricated mutation-surface score,
// in both registers.
#[test]
fn audit_reports_insufficient_data_without_observe_captured_signal() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("InsufficientData"))
        .stdout(predicate::str::contains(INSUFFICIENT_REASON));

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["behavioral_signal"]["observations"], 0);
    assert_eq!(parsed["insufficient_data"]["reason"], INSUFFICIENT_REASON);
    let items = parsed["items"].as_array().expect("items");
    assert!(
        !items.iter().any(|i| i["kind"] == "mutation_surface"),
        "no fabricated behavioral score without observe-captured held-out signal"
    );
}

// Once enough observe-captured sessions accumulate AND the repo indexes into
// a real symbol graph, the behavioral item lights up with a measured (not
// fabricated) cost.
#[test]
fn audit_lights_up_behavioral_metrics_once_corpus_is_sufficient() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);
    std::fs::write(
        repo.path().join("app.py"),
        "def handle(x):\n    return store(x)\n\ndef store(x):\n    return x\n",
    )
    .expect("write indexable source");

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(
        parsed["behavioral_signal"]["observations"],
        MIN_HELD_OUT_OBSERVATIONS
    );
    assert!(parsed.get("insufficient_data").is_none());
    let items = parsed["items"].as_array().expect("items");
    let surface = items
        .iter()
        .find(|i| i["kind"] == "mutation_surface")
        .expect("sufficient corpus re-enables the behavioral item");
    assert!(
        surface["measured_cost"]["value"].as_u64().unwrap() > 0,
        "the cost is measured from the repo's own graph: {surface}"
    );
}

// aoa-d6t.23 review finding: a sufficient corpus over a repo that indexes to
// an empty graph must not resurrect the fabricated '0 writable files
// reachable' score — no graph means no measurement, so no item.
#[test]
fn audit_withholds_the_surface_score_when_nothing_indexes() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(
        parsed["behavioral_signal"]["observations"],
        MIN_HELD_OUT_OBSERVATIONS
    );
    let items = parsed["items"].as_array().expect("items");
    assert!(
        !items.iter().any(|i| i["kind"] == "mutation_surface"),
        "an empty graph measures nothing; no fabricated score"
    );
}

// The reviewers' probe (aoa-d6t.23): a full window's worth of blank
// live-*.jsonl files must NOT satisfy the behavioral window — the precondition
// measures held-out signal, not session-file count.
#[test]
fn audit_ignores_contentless_sessions_when_counting_observations() {
    let repo = TempDir::new().expect("tempdir");
    let traces = repo.path().join(".aoa").join("traces");
    std::fs::create_dir_all(&traces).expect("create traces dir");
    for i in 0..MIN_HELD_OUT_OBSERVATIONS {
        std::fs::write(traces.join(format!("live-s{i}.jsonl")), "").expect("write blank log");
    }

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["behavioral_signal"]["observations"], 0);
    assert_eq!(parsed["insufficient_data"]["reason"], INSUFFICIENT_REASON);
    let items = parsed["items"].as_array().expect("items");
    assert!(
        !items.iter().any(|i| i["kind"] == "mutation_surface"),
        "blank sessions must not re-enable the behavioral item"
    );
}

// recommend with no observe-captured held-out signal: the determination tags
// the behavioral metrics InsufficientData (not Advisory) and the note carries
// the reason.
#[test]
fn recommend_reports_insufficient_data_without_observe_captured_signal() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["recommend", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("InsufficientData"))
        .stdout(predicate::str::contains(INSUFFICIENT_REASON));

    let output = aoa()
        .args(["recommend", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let note = &parsed["insufficient_data"];
    assert_eq!(note["reason"], INSUFFICIENT_REASON);
    let metrics = note["metrics"].as_array().expect("metrics");
    assert_eq!(metrics.len(), 4, "the four locality metrics");
    assert!(metrics.iter().any(|m| m == "retrieval_locality"));
}

// recommend with a sufficient corpus carries no InsufficientData note.
#[test]
fn recommend_omits_insufficient_data_with_a_sufficient_corpus() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);
    let output = aoa()
        .args(["recommend", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed.get("insufficient_data").is_none());
}

// eval run: the report counts its held-out observations against the window and
// carries the InsufficientData note (the fixture run has only two trials).
#[test]
fn eval_run_reports_insufficient_data_below_the_window() {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["behavioral_signal"]["observations"], 2);
    assert_eq!(parsed["insufficient_data"]["reason"], INSUFFICIENT_REASON);

    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .assert()
        .stdout(predicate::str::contains(INSUFFICIENT_REASON));
}

// R7 keystone end-to-end: the reproduction-before-mutation gate blocks a write
// when no test ran, and allows it once a reproduction span is recorded.
//
// These exercise stdin, so they use `assert_cmd::Command` (which has
// `write_stdin`) rather than the `std::process::Command` helper above.
fn aoa_stdin() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("aoa").expect("aoa binary builds")
}

/// Run `aoa observe --repo <repo> --enforce`, asserting success.
fn observe_enforce(repo: &Path) {
    aoa_stdin()
        .args(["observe", "--repo", repo.to_str().unwrap(), "--enforce"])
        .assert()
        .success();
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

/// The live log the enforce hooks append to for `hook_payload`'s session.
fn live_log_path(repo: &Path) -> std::path::PathBuf {
    repo.join(".aoa/traces/live-it-session.jsonl")
}

fn live_log(repo: &Path) -> String {
    std::fs::read_to_string(live_log_path(repo)).expect("hooks created a live log")
}

/// The acceptance criterion, driven through the real CLI: each of the four
/// non-landing outcomes is recorded and observable, and none of them is the
/// span that edit ground truth is derived from.
///
/// Every outcome is a separate hook subcommand because the host raises a
/// separate event for each. Nothing here inspects a tool response.
#[test]
fn enforce_records_each_write_outcome_under_its_own_span() {
    for (subcommand, expected) in [
        ("commit", "write.committed"),
        ("fail", "write.failed"),
        ("deny", "write.denied"),
    ] {
        let repo = TempDir::new().unwrap();
        aoa_stdin()
            .args(["enforce", subcommand])
            .write_stdin(hook_payload("Edit", None, repo.path()))
            .assert()
            .success();

        let contents = live_log(repo.path());
        assert!(
            contents.contains(&format!(r#""type":"{expected}""#)),
            "`enforce {subcommand}` must record {expected}: {contents}"
        );
        assert!(
            contents.contains("src/lib.rs"),
            "{expected} carries its target path: {contents}"
        );
    }
}

/// An outcome hook reports history. It must never fail the tool call after the
/// fact, and it must ignore a tool it does not guard even if a stale or
/// hand-edited settings.json routes one to it.
#[test]
fn enforce_outcome_hooks_never_block_and_ignore_unguarded_tools() {
    let repo = TempDir::new().unwrap();
    aoa_stdin()
        .args(["enforce", "commit"])
        .write_stdin(hook_payload("Bash", Some("ls"), repo.path()))
        .assert()
        .success();

    assert!(
        !live_log_path(repo.path()).exists(),
        "a non-mutation tool records no write outcome"
    );
}

/// The full lifecycle of one edit: the gate allows it and logs intent, then the
/// host's success event lands the confirmation. Both spans coexist — the
/// attempt is kept for the intent-versus-outcome signal, while only the
/// committed span is ground truth.
#[test]
fn allowed_write_records_intent_then_confirmation() {
    let repo = TempDir::new().unwrap();
    // A reproduction first, so the R7 gate permits the write.
    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("pytest -q"), repo.path()))
        .assert()
        .success();
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Edit", None, repo.path()))
        .assert()
        .success();
    aoa_stdin()
        .args(["enforce", "commit"])
        .write_stdin(hook_payload("Edit", None, repo.path()))
        .assert()
        .success();

    let contents = live_log(repo.path());
    assert!(contents.contains(r#""type":"write.attempt""#), "{contents}");
    assert!(
        contents.contains(r#""type":"write.committed""#),
        "{contents}"
    );
    assert!(
        contents.find(r#""write.attempt""#) < contents.find(r#""write.committed""#),
        "intent is recorded before the outcome that settles it: {contents}"
    );
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

    // The live log carries the test.run, the earlier write.blocked span, and
    // the allowed write recorded as write.attempt with its target path — the
    // held-out ground truth the live corpus accumulates (aoa-d6t.23).
    let contents = live_log(repo.path());
    assert!(contents.contains("test.run"));
    assert!(contents.contains("write.blocked"));
    assert!(
        contents.contains(r#""type":"write.attempt""#),
        "allowed write must land as write.attempt: {contents}"
    );
    assert!(
        contents.contains(r#""path":"src/lib.rs""#),
        "write.attempt carries its target path: {contents}"
    );
}

// An allowed write is recorded even when policy disables the reproduction
// gate: held-out truth capture is independent of gating (aoa-d6t.23).
#[test]
fn enforce_check_records_allowed_write_when_reproduction_is_disabled() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n",
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Edit", None, repo.path()))
        .assert()
        .success();

    let contents = live_log(repo.path());
    assert!(
        contents.contains(r#""type":"write.attempt""#) && contents.contains("src/lib.rs"),
        "allowed write recorded with its path: {contents}"
    );
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

    observe_enforce(repo.path());
    let first = std::fs::read_to_string(&settings).expect("settings written");
    assert!(first.contains("aoa enforce check"));
    assert!(first.contains("aoa enforce record"));

    // Re-running is byte-stable (idempotent merge).
    observe_enforce(repo.path());
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

/// The audit's runtime-hook plane check is existence-only, so without this
/// test a change to the generated hook entries would silently strand the
/// repo's committed `.claude/settings.json` (aoa-vrx.3) on the old shape
/// while the gating self-audit kept passing.
#[test]
fn tracked_settings_json_matches_observe_enforce_output() {
    let fresh = TempDir::new().unwrap();
    observe_enforce(fresh.path());
    let generated = std::fs::read_to_string(fresh.path().join(".claude/settings.json"))
        .expect("observe --enforce writes settings.json");

    let tracked_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".claude/settings.json");
    let tracked = std::fs::read_to_string(&tracked_path)
        .expect("repo-root .claude/settings.json is tracked (runtime-hook plane)");

    assert_eq!(
        tracked, generated,
        "tracked .claude/settings.json drifted from `aoa observe --enforce` output; \
         regenerate it with `cargo run -p aoa -- observe --repo . --enforce`"
    );
}

// --- aoa gap checkbox-baseline (aoa-d6t.28) ---------------------------------

/// A fixture tree passing all four level-1 mechanical criteria, so the
/// checkbox baseline lands at level 2 (Documented).
fn checkbox_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let p = dir.path();
    std::fs::write(p.join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(p.join("README.md"), "# demo\n").unwrap();
    std::fs::write(p.join("clippy.toml"), "").unwrap();
    std::fs::create_dir(p.join("tests")).unwrap();
    dir
}

// The JSON register emits the full CheckboxBaseline artifact — repo id, pinned
// criteria version, achieved level, and every criterion including excluded
// ones with their reasons — so study repos can be scored in batch.
#[test]
fn gap_checkbox_baseline_json_emits_full_artifact() {
    let repo = checkbox_repo();
    let output = aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo", "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["repo_id"], "demo-repo");
    assert_eq!(parsed["level"], 2);
    assert!(parsed["criteria_version"]
        .as_str()
        .unwrap()
        .starts_with("factory-provisional-"));
    let criteria = parsed["criteria"].as_array().expect("criteria array");
    let excluded: Vec<&Value> = criteria
        .iter()
        .filter(|c| c["status"]["status"] == "excluded")
        .collect();
    assert!(
        !excluded.is_empty(),
        "excluded criteria are carried in JSON, never dropped"
    );
    for c in &excluded {
        assert!(
            !c["status"]["reason"].as_str().unwrap().is_empty(),
            "every excluded criterion carries its reason"
        );
    }
}

// Default human register: achieved level with its Factory name, per-level and
// per-pillar tallies, and the excluded COUNT — but not the reason prose.
#[test]
fn gap_checkbox_baseline_human_summarizes_levels_and_pillars() {
    let repo = checkbox_repo();
    let assert = aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo-repo"))
        .stdout(predicate::str::contains("level 2 (Documented)"))
        .stdout(predicate::str::contains("style_and_validation"));
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(
        !stdout.contains("hosting-platform setting"),
        "exclusion reasons stay behind --show-excluded"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "no ANSI escapes when stdout is not a terminal"
    );
}

// --show-excluded surfaces every excluded criterion with its reason in the
// human register; the JSON register carries them regardless (identical
// findings across registers).
#[test]
fn gap_checkbox_baseline_show_excluded_lists_reasons() {
    let repo = checkbox_repo();
    aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo", "--show-excluded"])
        .assert()
        .success()
        .stdout(predicate::str::contains("branch_protection"))
        .stdout(predicate::str::contains("hosting-platform setting"))
        .stdout(predicate::str::contains("secret_scanning"))
        .stdout(predicate::str::contains("self_improving_orchestration"));
}

// A missing root fails loud with a typed error, never a level-1 default.
#[test]
fn gap_checkbox_baseline_missing_root_fails_loud() {
    let repo = TempDir::new().expect("tempdir");
    let missing = repo.path().join("nope");
    aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(&missing)
        .args(["--repo-id", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a directory"));
}

// `--json` is global on `aoa gap`: placed before the subcommand (the position
// that works for bare `aoa gap --json`) it must still select the JSON register,
// never parse-and-ignore into human text.
#[test]
fn gap_parent_json_flag_reaches_checkbox_baseline() {
    let repo = checkbox_repo();
    let output = aoa()
        .args(["gap", "--json", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .expect("parent-position --json emits the CheckboxBaseline artifact");
    assert_eq!(parsed["repo_id"], "demo-repo");
}

// The bare `aoa gap` surface (no subcommand) still renders the R9c
// determination — the new subcommand must not displace it.
#[test]
fn gap_without_subcommand_still_renders_determination() {
    aoa()
        .args(["gap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("construct validity"));
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

// --- aoa report (aoa-d6t.19, leg 1) ------------------------------------------
// One end-to-end operator readiness view composing the audit punch-list, the
// R9c Advisory/Gating determination, the migration registry, the recommend
// join, and (when present) the R0 falsification verdict.

#[test]
fn report_composes_all_pillars_and_reports_absent_falsification() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["report", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("punch-list"))
        .stdout(predicate::str::contains("construct validity"))
        .stdout(predicate::str::contains("navigability-anchor"))
        .stdout(predicate::str::contains("AOA recommendations"))
        // Absent input is reported as absent, never fabricated.
        .stdout(predicate::str::contains("falsification.json: absent"));
}

#[test]
fn report_json_composes_pillars_and_pillar_is_not_live_without_falsification() {
    let repo = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed["audit"]["items"].is_array());
    assert!(parsed["construct_validity"]["metrics"].is_array());
    assert!(
        !parsed["migrations"]
            .as_array()
            .expect("migrations")
            .is_empty(),
        "the migration registry is surfaced"
    );
    assert!(parsed["recommendations"]["items"].is_array());
    assert_eq!(parsed["falsification"]["status"], "absent");
    assert_eq!(parsed["migrate_pillar_live"], false);
}

// aoa-d6t.35: `report` composes audit + determination + recommend into ONE
// document, so it must condition the determination on the same behavioral
// signal the audit measured — otherwise the halves contradict each other.
#[test]
fn report_json_conditions_the_determination_on_the_behavioral_signal() {
    let repo = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    // The audit half already reports the shortfall.
    assert_eq!(
        parsed["audit"]["insufficient_data"]["reason"],
        INSUFFICIENT_REASON
    );

    // The determination must agree: all four behavioral metrics tagged
    // insufficient_data, never advisory.
    let metrics = parsed["construct_validity"]["metrics"]
        .as_array()
        .expect("metrics");
    let insufficient = metrics
        .iter()
        .filter(|m| m["mode"] == "insufficient_data")
        .count();
    assert_eq!(insufficient, 4, "the four locality metrics: {metrics:#?}");
    assert!(
        metrics
            .iter()
            .any(|m| m["metric"] == "retrieval_locality" && m["mode"] == "insufficient_data"),
        "retrieval_locality must not be advisory on a greenfield repo: {metrics:#?}"
    );

    // And the recommend join carries the note, as `aoa recommend` does.
    assert_eq!(
        parsed["recommendations"]["insufficient_data"]["reason"],
        INSUFFICIENT_REASON
    );
}

// The mirror: once the corpus crosses the window, `report` stops reporting the
// shortfall — the conditioning is a real precondition, not a constant.
#[test]
fn report_json_omits_insufficient_data_with_a_sufficient_corpus() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), 10);
    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    assert!(parsed["audit"].get("insufficient_data").is_none());
    assert!(parsed["recommendations"].get("insufficient_data").is_none());
    let metrics = parsed["construct_validity"]["metrics"]
        .as_array()
        .expect("metrics");
    assert!(
        !metrics.iter().any(|m| m["mode"] == "insufficient_data"),
        "a sufficient corpus leaves no metric in insufficient_data: {metrics:#?}"
    );
}

// aoa-d6t.41: `report` must audit through the same repo-aware config as `audit`
// and `recommend`. Building an `AuditConfig::default()` here hands the audit an
// empty symbol graph, which silently withholds the measured mutation-surface
// item — the operator-facing readiness view under-reporting the very findings
// the other two surfaces report for the same repo.
//
// The repo needs BOTH a sufficient corpus and indexable source: without the
// corpus this is the InsufficientData path, and without a .py file the graph is
// empty for every command, so neither alone would catch the divergence.
#[test]
fn report_and_audit_agree_on_findings_for_an_indexable_repo() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);
    std::fs::write(
        repo.path().join("app.py"),
        "def handle(x):\n    return store(x)\n\ndef store(x):\n    return x\n",
    )
    .expect("write indexable source");

    let kinds = |args: &[&str]| -> Vec<String> {
        let output = aoa().args(args).arg(repo.path()).output().expect("run");
        let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
        // `audit --json` is the AuditReport itself; `report --json` nests it.
        match parsed.get("audit") {
            Some(audit) => audit["items"].as_array(),
            None => parsed["items"].as_array(),
        }
        .expect("items")
        .iter()
        .map(|i| i["kind"].as_str().expect("kind").to_string())
        .collect()
    };

    let from_audit = kinds(&["audit", "--json", "--repo"]);
    let from_report = kinds(&["report", "--json", "--repo"]);

    assert!(
        from_audit.iter().any(|k| k == "mutation_surface"),
        "precondition: the fixture repo indexes into a real graph: {from_audit:?}"
    );
    assert_eq!(
        from_report, from_audit,
        "`report` must not drop findings `audit` reports for the same repo"
    );
}

#[test]
fn report_proceed_verdict_marks_the_migrate_pillar_live() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("falsification.json"),
        r#"{"verdict":"proceed","notes":[]}"#,
    )
    .unwrap();

    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["falsification"]["status"], "present");
    assert_eq!(parsed["falsification"]["verdict"], "proceed");
    assert_eq!(parsed["migrate_pillar_live"], true);
}

#[test]
fn report_pivot_verdict_keeps_the_migrate_pillar_not_live() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("falsification.json"),
        r#"{"verdict":"pivot","notes":[]}"#,
    )
    .unwrap();

    aoa()
        .args(["report", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("pivot"))
        .stdout(predicate::str::contains("not live"));
}

#[test]
fn report_precondition_unmet_verdict_is_surfaced_and_not_live() {
    // An inconclusive written by an unmet precondition (e.g. too_few_repos)
    // carries its discriminator; the report surfaces it and the pillar stays
    // not live.
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("falsification.json"),
        r#"{"verdict":"inconclusive","precondition_unmet":"too_few_repos","notes":[]}"#,
    )
    .unwrap();

    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(
        parsed["falsification"]["precondition_unmet"],
        "too_few_repos"
    );
    assert_eq!(parsed["migrate_pillar_live"], false);
}

#[test]
fn report_fails_loud_on_malformed_falsification_json() {
    // A present-but-unparsable falsification.json is a hard error, never
    // silently treated as absent (that would fabricate "gate never ran").
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(repo.path().join("falsification.json"), "not json").unwrap();

    aoa()
        .args(["report", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("falsification.json"));
}

// --- aoa audit --self (aoa-d6t.19, leg 2: R14 lint-thyself) -------------------
// The toolkit measures its own added context tokens (the files its applied
// migration wrote, before vs after) and flags a regression when the median
// rose without demonstrated held-out gain.

#[test]
fn audit_self_without_migration_reports_absent_and_exits_zero() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no applied migration"));
}

#[test]
fn audit_self_flags_regression_when_context_rose_without_heldout_evidence() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a flagged regression exits non-zero"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["status"], "measured");
    assert_eq!(parsed["median_before_tokens"], 0.0);
    assert!(parsed["median_after_tokens"].as_f64().expect("median") > 0.0);
    assert_eq!(parsed["held_out"]["status"], "absent");
    assert_eq!(parsed["context_regression"], true);
}

#[test]
fn audit_self_human_names_the_regression() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("context regression"))
        .stdout(predicate::str::contains("held-out evidence: absent"));
}

#[test]
fn audit_self_with_heldout_gain_clears_the_regression() {
    // The fixture pair yields held_out_delta > 0 (label good in `eval compare`),
    // so the context rise is justified and no regression is flagged.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["held_out"]["status"], "present");
    assert!(
        parsed["held_out"]["held_out_delta"]
            .as_f64()
            .expect("delta")
            > 0.0
    );
    assert_eq!(parsed["context_regression"], false);
}

#[test]
fn audit_self_baseline_without_migrated_is_rejected() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .assert()
        .failure();
}

#[test]
fn audit_baseline_without_self_is_rejected() {
    // The held-out pair only means something to the self-audit; supplying it to
    // a plain audit is a usage error, not silently ignored.
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .assert()
        .failure();
}

/// Write a hand-crafted migration manifest under `repo`, as a corrupted or
/// hostile checkout could contain.
fn write_migration_manifest(repo: &Path, manifest: &Value) {
    std::fs::create_dir_all(repo.join(".aoa/migrate")).unwrap();
    std::fs::write(
        repo.join(".aoa/migrate/manifest.json"),
        manifest.to_string(),
    )
    .unwrap();
}

#[test]
fn audit_self_resolves_manifest_paths_against_repo_not_cwd() {
    // `aoa migrate --apply` with a relative `--repo` (the default is `.`)
    // records entry paths like `./README.md` verbatim. The self-audit must
    // resolve them against `--repo`, not the audit process's cwd — otherwise
    // it silently tokenizes an unrelated same-named file (fabricated counts)
    // or hard-errors on a perfectly intact migration record.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo", "."])
        .current_dir(repo.path())
        .assert()
        .success();

    // Audit from an unrelated cwd that contains a decoy README.md.
    let elsewhere = TempDir::new().expect("tempdir");
    std::fs::write(elsewhere.path().join("README.md"), "x").unwrap();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .current_dir(elsewhere.path())
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "no held-out evidence, so the rise is still a regression; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let readme = parsed["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|f| f["path"].as_str().expect("path").ends_with("README.md"))
        .expect("README.md entry");
    assert!(
        readme["after_tokens"].as_u64().expect("after_tokens") > 1,
        "must measure the migration-written README, not the 1-byte decoy: {readme}"
    );
}

#[test]
fn audit_self_escapes_manifest_text_in_human_output() {
    // Entry paths and fix ids come verbatim from on-disk manifest JSON — the
    // same trust level as falsification.json, whose fields the report escapes.
    // A crafted path or fix id must not put a raw ESC byte on the terminal.
    let repo = TempDir::new().expect("tempdir");
    let evil_name = "evil\u{1b}[2Jfile.md";
    std::fs::write(repo.path().join(evil_name), "hello world").unwrap();
    write_migration_manifest(
        repo.path(),
        &serde_json::json!({
            "fixes_applied": ["fix\u{1b}[31mred"],
            "entries": [{ "action": "created", "path": repo.path().join(evil_name) }],
        }),
    );

    let output = aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('\u{1b}'),
        "raw ESC reached the terminal: {stdout:?}"
    );
    assert!(
        stdout.contains("\\u{1b}"),
        "escaped form must be shown: {stdout:?}"
    );
}

#[test]
fn audit_self_rejects_manifest_entry_outside_the_repo() {
    // Apply validates every target stays inside the repo, so an entry that
    // resolves outside it is a corrupted (or hostile) migration record — a
    // hard error, never a read of arbitrary reachable files (/dev/zero,
    // /etc/passwd) on the manifest's say-so.
    let repo = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("tempdir");
    let secret = outside.path().join("secret.md");
    std::fs::write(&secret, "outside the audited repo").unwrap();
    write_migration_manifest(
        repo.path(),
        &serde_json::json!({
            "fixes_applied": ["r14-anchor"],
            "entries": [{ "action": "created", "path": secret }],
        }),
    );

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("outside the repo"));
}

#[test]
fn audit_self_caps_migration_written_file_reads() {
    // Migration-written files are read through the same byte cap as every
    // other untrusted file the CLI touches; a pathological entry cannot
    // exhaust memory.
    let repo = TempDir::new().expect("tempdir");
    let big = repo.path().join("big.md");
    std::fs::write(&big, vec![b'x'; 16 * 1024 * 1024 + 1]).unwrap();
    write_migration_manifest(
        repo.path(),
        &serde_json::json!({
            "fixes_applied": ["r14-anchor"],
            "entries": [{ "action": "created", "path": big }],
        }),
    );

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("byte cap"));
}

#[test]
fn audit_self_does_not_accept_a_not_good_gain_as_justification() {
    // held-out rose (+0.25) but the reward-hacking gap widened (+0.5): `aoa
    // eval compare` labels this pair not_good. The self-audit must consume
    // that same gate — a not_good pair cannot justify a context rise — and
    // surface the label in the JSON register.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let runs = TempDir::new().expect("tempdir");
    let baseline = runs.path().join("baseline.json");
    let migrated = runs.path().join("migrated.json");
    // baseline: visible 0.25, held-out 0.25 (gap 0).
    std::fs::write(
        &baseline,
        r#"{"tasks":[
            {"visible_success":true,"held_out_success":true},
            {"visible_success":false,"held_out_success":false},
            {"visible_success":false,"held_out_success":false},
            {"visible_success":false,"held_out_success":false}],
            "held_out_provenance":"native_composed","canaries":[]}"#,
    )
    .unwrap();
    // migrated: visible 1.0, held-out 0.5 (gap 0.5) — gap widened.
    std::fs::write(
        &migrated,
        r#"{"tasks":[
            {"visible_success":true,"held_out_success":true},
            {"visible_success":true,"held_out_success":true},
            {"visible_success":true,"held_out_success":false},
            {"visible_success":true,"held_out_success":false}],
            "held_out_provenance":"native_composed","canaries":[]}"#,
    )
    .unwrap();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(&baseline)
        .arg("--migrated")
        .arg(&migrated)
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a not_good pair must not clear the regression"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["held_out"]["label"], "not_good");
    assert_eq!(parsed["context_regression"], true);
}

#[test]
fn audit_self_surfaces_zero_canary_leak_shape_warning() {
    // The fixture pair is leak-shaped with zero canaries: `aoa eval compare`
    // warns zero_canary_leak_shape so the contamination-shaped gain never
    // passes silently. The self-audit consumes the same comparison and must
    // surface the warning in both registers even though the good label still
    // clears the regression.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["held_out"]["label"], "good");
    assert_eq!(
        parsed["held_out"]["warnings"][0], "zero_canary_leak_shape",
        "the leak-shaped warning must not be dropped: {parsed}"
    );

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains("ZeroCanaryLeakShape"));
}
