use super::eval_run::{run_dir, tasks_dir};
use super::report::seeded_indexable_repo;
use super::*;

// --- aoa-d6t.23: greenfield/cold-start InsufficientData across the surfaces ---

pub(super) const INSUFFICIENT_REASON: &str = "no held-out behavioral signal for this repo yet";

/// Accumulate `n` observe-captured live sessions under `<repo>/.aoa/traces/`,
/// each carrying a landed edit — a session counts as a held-out behavioral
/// observation only when it holds a real edit out.
///
/// Each session records the full write lifecycle the hooks emit: the
/// `write.attempt` logged before the tool runs, then the `write.committed`
/// logged once it succeeds. The attempt alone would not do, because an attempt
/// is not a landed edit.
pub(super) fn seed_live_sessions(repo: &Path, n: usize) {
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

pub(super) fn seed_live_sessions_with_spans(repo: &Path, n: usize, spans: &str) {
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
    let repo = seeded_indexable_repo();

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
