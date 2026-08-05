use std::path::Path;

use aoa_falsify_build::{build, Manifest};

const COMMIT: &str = r#"{"algorithm":"sha1","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn repo(runs: &str, extra: &str) -> String {
    format!(
        r#"{{"repo_id":"sample/repo","repo_commit":{COMMIT},"confidence":"high","exposure":"unexposed",
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

fn write_answer_task(tasks_dir: &Path, task_id: &str) {
    let task_dir = tasks_dir.join(task_id);
    std::fs::create_dir_all(task_dir.join("tests")).unwrap();
    std::fs::write(
        task_dir.join("task.toml"),
        format!("[task]\nid = \"{task_id}\"\nrepo = \"sample/repo\"\n"),
    )
    .unwrap();
    std::fs::write(task_dir.join("instruction.md"), "answer the question").unwrap();
    std::fs::write(
        task_dir.join("tests/ground_truth.json"),
        r#"{"answer":["src/pkg/app.py"],"answer_type":"file_list","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    )
    .unwrap();
}

fn write_answer_trial(base_dir: &Path, run_dir: &str, task_id: &str) {
    let trial_dir = base_dir.join(run_dir).join(task_id);
    std::fs::create_dir_all(&trial_dir).unwrap();
    std::fs::write(
        trial_dir.join("scoring.json"),
        r#"{"scorer_family":"dual_composite","passed_direct":true,"passed_artifact":true}"#,
    )
    .unwrap();
    std::fs::write(
        trial_dir.join("agent_output.txt"),
        concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/work/checkout/src/pkg/app.py"}}]}}"#,
            "\n"
        ),
    )
    .unwrap();
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

#[test]
fn divergent_identical_pair_sets_fail_with_missing_and_extra_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = temp.path().join("tasks");
    write_answer_task(&tasks_dir, "alpha");
    write_answer_task(&tasks_dir, "beta");
    for (seed, task_id) in [(1, "alpha"), (2, "beta"), (3, "alpha")] {
        write_answer_trial(temp.path(), &format!("seed{seed}/repo"), task_id);
        write_answer_trial(temp.path(), &format!("seed{seed}/harness"), task_id);
    }
    std::fs::write(temp.path().join("repo.json"), "{}").unwrap();
    std::fs::write(temp.path().join("harness.json"), "{}").unwrap();
    std::fs::write(
        temp.path().join("calibration.json"),
        r#"{"method":"external_outcome_correlation","protocol_version":"r11","corpus_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","sample_size":20,"criteria":["rho-significant"],"conclusion":"calibrated"}"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("index.json"),
        r#"{"documents":[{"relative_path":"src/pkg/app.py","occurrences":[{"symbol":"pkg/app#app().","roles":["definition"]}]}],"aoa":{"writable":[]}}"#,
    )
    .unwrap();
    let runs = r#"
        {"seed":1,"repo_arm":"seed1/repo","harness_arm":"seed1/harness"},
        {"seed":2,"repo_arm":"seed2/repo","harness_arm":"seed2/harness"},
        {"seed":3,"repo_arm":"seed3/repo","harness_arm":"seed3/harness"}"#;
    let manifest = manifest(&repo(
        runs,
        r#""task_shape":"answer","scip_index":"index.json","#,
    ));

    let error = build(&manifest, &tasks_dir, temp.path())
        .unwrap_err()
        .to_string();

    assert!(error.contains("run 1 (seed 2)"), "got: {error}");
    assert!(error.contains(r#"missing ["alpha"]"#), "got: {error}");
    assert!(error.contains(r#"extra ["beta"]"#), "got: {error}");
}
