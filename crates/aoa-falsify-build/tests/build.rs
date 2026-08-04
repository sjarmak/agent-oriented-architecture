use std::path::Path;

use aoa_falsify_build::{build, Manifest};

const COMMIT: &str = r#"{"algorithm":"sha1","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn repo(runs: &str, extra: &str) -> String {
    format!(
        r#"{{"repo_id":"sample/repo","repo_commit":{COMMIT},"confidence":"high",
        "calibration_artifact":"calibration.json","repo_arm_config":"repo.json",
        "harness_arm_config":"harness.json",{extra}"runs":[{runs}]}}"#
    )
}

fn manifest(repos: &str) -> Manifest {
    serde_json::from_str(&format!(
        r#"{{"k_runs":3,"min_holdout_size":1,"repos":[{repos}]}}"#
    ))
    .unwrap()
}

fn build_error(manifest: &Manifest, base_dir: &Path) -> String {
    build(manifest, Path::new("tasks"), base_dir)
        .unwrap_err()
        .to_string()
}

#[test]
fn an_empty_manifest_fails_loud_at_the_library_boundary() {
    let manifest: Manifest =
        serde_json::from_str(r#"{"k_runs":3,"min_holdout_size":1,"repos":[]}"#).unwrap();

    let error = build(&manifest, Path::new("tasks"), Path::new(".")).unwrap_err();

    assert_eq!(error.to_string(), "manifest declares no repos");
}

#[test]
fn answer_shape_requires_an_index() {
    let manifest = manifest(&repo("", r#""task_shape":"answer","#));

    let error = build_error(&manifest, Path::new("."));

    assert!(error.contains("requires scip_index"), "got: {error}");
}

#[test]
fn public_errors_escape_operator_and_filesystem_control_characters() {
    let repo =
        repo("", r#""task_shape":"answer","#).replace("sample/repo", "sample/\\u001b[2Jrepo");
    let manifest = manifest(&repo);

    let error = build_error(&manifest, Path::new("."));

    assert!(!error.contains('\u{001b}'), "raw ESC leaked: {error:?}");
    assert!(
        error.contains(r#"\u{1b}"#),
        "escaped value missing: {error}"
    );
}

#[test]
fn mixed_task_shapes_fail_before_evidence_is_read() {
    let answer = repo("", r#""task_shape":"answer","scip_index":"index.json","#);
    let edit = repo("", "");
    let manifest = manifest(&format!("{answer},{edit}"));

    let error = build_error(&manifest, Path::new("."));

    assert!(error.contains("manifest mixes task shapes"), "got: {error}");
}

#[test]
fn duplicate_seeds_are_not_independent_replications() {
    let runs = r#"
        {"seed":1,"repo_arm":"a","harness_arm":"b"},
        {"seed":1,"repo_arm":"c","harness_arm":"d"},
        {"seed":3,"repo_arm":"e","harness_arm":"f"}"#;
    let manifest = manifest(&repo(runs, ""));

    let error = build_error(&manifest, Path::new("."));

    assert!(error.contains("seed 1 is used by more than one run"));
}

#[test]
fn reused_run_directories_are_not_independent_replications() {
    let runs = r#"
        {"seed":1,"repo_arm":"a","harness_arm":"b"},
        {"seed":2,"repo_arm":"a","harness_arm":"d"},
        {"seed":3,"repo_arm":"e","harness_arm":"f"}"#;
    let manifest = manifest(&repo(runs, ""));

    let error = build_error(&manifest, Path::new("."));

    assert!(error.contains("used by more than one run/arm"));
}

#[test]
fn aliased_run_directories_are_rejected_by_physical_identity() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("seed1/repo")).unwrap();
    std::fs::create_dir_all(temp.path().join("seed2")).unwrap();
    let alias = temp.path().join("seed2/../seed1/repo");
    let runs = format!(
        r#"
        {{"seed":1,"repo_arm":"{}","harness_arm":"b"}},
        {{"seed":2,"repo_arm":"{}","harness_arm":"d"}},
        {{"seed":3,"repo_arm":"e","harness_arm":"f"}}"#,
        temp.path().join("seed1/repo").display(),
        alias.display(),
    );
    let manifest = manifest(&repo(&runs, ""));

    let error = build_error(&manifest, temp.path());

    assert!(error.contains("used by more than one run/arm"));
}
