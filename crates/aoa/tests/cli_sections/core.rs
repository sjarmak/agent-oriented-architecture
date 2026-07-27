use super::*;

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

#[cfg(unix)]
#[test]
fn cli_error_boundary_neutralises_terminal_controls_from_paths() {
    let dir = TempDir::new().expect("tempdir");
    let hostile = dir.path().join("repo\u{1b}[2Jname");
    let output = aoa()
        .args(["audit", "--repo"])
        .arg(&hostile)
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(!stderr.contains('\u{1b}'), "raw ESC leaked: {stderr:?}");
    assert!(
        stderr.contains("\\u{1b}[2J"),
        "escaped path remains diagnosable: {stderr:?}"
    );
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

/// The same convention one seam over, now covering every command that loads JSON
/// through `load_json_capped` (aoa-empz). `eval compare` is the representative
/// caller. This has to stay a CLI test: the doubled path shows up only under the
/// `{err:#}` chain rendering `main` uses, which a `to_string()` unit test on the
/// helper would never see.
#[test]
fn missing_run_file_error_names_the_path_once() {
    let missing = "/nonexistent/aoa-empz-baseline.json";
    let output = aoa()
        .args(["eval", "compare", missing, missing])
        .output()
        .expect("run");
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches(missing).count(),
        1,
        "diagnostic must name the offending file exactly once: {stderr}"
    );
    assert!(
        stderr.contains("failed to read run file"),
        "diagnostic must still name the file class: {stderr}"
    );
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
