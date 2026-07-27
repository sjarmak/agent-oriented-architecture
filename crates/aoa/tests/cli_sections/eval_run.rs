use super::*;

// --- aoa-2lw: eval run post-processes a codeprobe run -------------------------

pub(super) fn run_dir() -> PathBuf {
    fixture("codeprobe_run")
}
pub(super) fn tasks_dir() -> PathBuf {
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

// aoa-qyo3: a numeric score is not held-out evidence when the scorer says it
// errored. Codeprobe persists 0.0/false alongside the error for verifier
// failures; treating those fallback values as a genuine failure silently
// contaminates the behavioral signal.
#[test]
fn eval_run_excludes_a_trial_whose_scorer_errored() {
    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("run");
    let id = "native-consensus-001";
    let trial = run.join(id);
    std::fs::create_dir_all(&trial).expect("trial dir");
    std::fs::copy(
        run_dir().join(id).join("agent_output.txt"),
        trial.join("agent_output.txt"),
    )
    .expect("copy transcript");
    std::fs::write(
        trial.join("scoring.json"),
        br#"{
            "score": 0.0,
            "passed": false,
            "error": "artifact verifier crashed",
            "verdict": "verifier_error"
        }"#,
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
        "a scorer error must fail the trial loud"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["record_count"], 0);
    assert_eq!(parsed["error_count"], 1);
    assert_eq!(
        parsed["behavioral_signal"]["observations"], 0,
        "fallback score 0.0 must not become held-out evidence"
    );
    let error = parsed["errors"][0]["error"].as_str().expect("error string");
    for expected in ["scoring.json", "artifact verifier crashed"] {
        assert!(
            error.contains(expected),
            "error must name {expected}: {error}"
        );
    }
}

// A trial dir whose name is not valid UTF-8 cannot become an addressable task
// id. Eval-run reports that trial as an error while preserving records from
// valid siblings; pairing callers retain discover_tasks' fail-closed behavior.
#[cfg(unix)]
#[test]
fn eval_run_isolates_a_non_utf8_trial_dir_name() {
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
        "the rejected trial must still make the batch fail"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid report JSON");
    assert_eq!(parsed["record_count"], 1);
    assert_eq!(parsed["error_count"], 1);
    let error = parsed["errors"][0]["error"]
        .as_str()
        .expect("rejected trial error");
    assert!(
        error.contains("not valid UTF-8"),
        "error must name the cause: {error}"
    );
    // The offending name reaches JSON escaped, never as raw bytes.
    assert!(
        !output.stdout.contains(&0xff),
        "raw invalid byte survived into output"
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
