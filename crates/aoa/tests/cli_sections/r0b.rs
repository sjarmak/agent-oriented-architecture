use super::eval_run::tasks_dir;
use super::*;

// --- aoa-2ce: R0b on live data — compose the leakage canary over codeprobe ----

pub(super) fn r0b_baseline() -> PathBuf {
    fixture("r0b_run_baseline")
}
pub(super) fn r0b_migrated() -> PathBuf {
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
