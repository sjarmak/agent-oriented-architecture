//! The enforcement plane's liveness surface, driven through the real CLI.
//!
//! AOA could already tell "hook set not installed" from "hook set out of date".
//! It could not tell **installed and enforcing** from **installed and silently
//! emitting nothing** — settings.json reads identically in both, and the absent
//! live log reads as no-activity rather than as broken instrumentation
//! (aoa-dpluh). These are the boundary tests for the three-state answer: the
//! silent state must be reported as silent and must never be reported as
//! enforcing.

use super::enforce::{aoa_stdin, hook_payload, observe_enforce};
use super::*;

/// `aoa audit --json` for `repo`, parsed.
fn audit_json(repo: &Path) -> Value {
    let output = aoa_stdin()
        .args(["audit", "--json", "--repo"])
        .arg(repo)
        .output()
        .expect("audit runs");
    serde_json::from_slice(&output.stdout).expect("audit emits JSON")
}

fn liveness_state(report: &Value) -> String {
    report["enforcement_liveness"]["state"]
        .as_str()
        .expect("the audit reports an enforcement-liveness state")
        .to_string()
}

/// Criterion (d), the sharp direction: hooks installed and nothing able to run
/// them. The plane must read as installed-but-silent, and must NOT read as
/// enforcing.
///
/// `observe --enforce` provisions `.aoa/traces/` as part of installing, so this
/// is also the case where the *directory* exists and holds no log — the shape
/// that used to be indistinguishable from a healthy repo between sessions.
#[test]
fn installed_hooks_that_never_ran_report_silent_and_never_enforcing() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    let report = audit_json(repo.path());
    assert_eq!(
        liveness_state(&report),
        "installed-but-silent",
        "installed hooks with no emitted span are silent, not enforcing: {report}"
    );
    assert_eq!(
        report["enforcement_liveness"]["silence"], "no-live-logs",
        "the traces dir exists and holds no live log; that is a distinct fact \
         from the dir being absent"
    );

    // (c) Never a pass: the silent plane is a Tier-1 finding on the punch-list,
    // so a consumer reading only `items` still cannot mistake it for healthy.
    let silent: Vec<&Value> = report["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter(|item| item["kind"] == "silent_plane")
        .collect();
    assert_eq!(
        silent.len(),
        1,
        "an installed-but-silent plane must raise exactly one finding: {report}"
    );
    assert_eq!(silent[0]["tier"], "tier-1");
    assert_eq!(silent[0]["plane"], "runtime-hook");
}

/// The negative direction of (d): a repo genuinely emitting spans reports
/// enforcing. Driven through the real `aoa enforce` hook path rather than a
/// hand-written file, so the test proves the surface reads what the installed
/// hooks actually write.
#[test]
fn a_repo_emitting_spans_through_the_hook_path_reports_enforcing() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("cargo test"), repo.path()))
        .assert()
        .success();

    let report = audit_json(repo.path());
    assert_eq!(
        liveness_state(&report),
        "enforcing",
        "a live log carrying a recorded span is the enforcing state: {report}"
    );
    assert_eq!(report["enforcement_liveness"]["spans"], 1);
    assert!(
        report["items"]
            .as_array()
            .expect("items array")
            .iter()
            .all(|item| item["kind"] != "silent_plane"),
        "an enforcing plane must raise no silence finding: {report}"
    );
}

/// (b) Loud, not blank. The silence has to be legible without parsing JSON —
/// the operator reading the human register is the one who missed this state for
/// a whole night of sessions.
#[test]
fn the_silent_state_is_loud_in_the_human_register() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    aoa_stdin()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("INSTALLED BUT SILENT"))
        .stdout(predicate::str::contains("NOT enforcing"));
}

/// A repo with no hook set at all is the third state, and it is distinct from
/// silence: there is nothing installed to be silent about, and the existing
/// missing-plane finding already covers it. Reporting silence here would tell an
/// operator to debug an install that was never done.
#[test]
fn a_repo_with_no_hook_set_reports_not_installed() {
    let repo = TempDir::new().unwrap();

    let report = audit_json(repo.path());
    assert_eq!(liveness_state(&report), "not-installed");
    let items = report["items"].as_array().expect("items array");
    assert!(
        items.iter().all(|item| item["kind"] != "silent_plane"),
        "an uninstalled plane raises the missing-plane finding, not silence: {report}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "missing_plane" && item["plane"] == "runtime-hook"),
        "the uninstalled runtime plane is still a missing-plane finding: {report}"
    );
}

/// (e) An absent `.aoa/traces` is not the same fact as an empty one. Both are
/// silence, but only one of them means the telemetry install itself never ran,
/// and an operator handed a single undifferentiated "silent" cannot tell which
/// thing to fix.
#[test]
fn an_absent_traces_directory_is_distinguished_from_an_empty_one() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    std::fs::remove_dir_all(repo.path().join(".aoa")).unwrap();

    let report = audit_json(repo.path());
    assert_eq!(liveness_state(&report), "installed-but-silent");
    assert_eq!(
        report["enforcement_liveness"]["silence"], "traces-directory-absent",
        "an absent traces dir must not read as an empty one: {report}"
    );
}
