use std::path::Path;

use aoa_falsify_build::{build, Manifest};

const COMMIT: &str = r#"{"algorithm":"sha1","hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn repo(runs: &str, extra: &str) -> String {
    format!(
        r#"{{"repo_id":"sample/repo","repo_commit":{COMMIT},"confidence":"high",
        "exposure_scan":"exposure.json",
        "calibration_artifact":"calibration.json","repo_arm_config":"repo.json",
        "harness_arm_config":"harness.json",{extra}"runs":[{runs}]}}"#
    )
}

/// A persisted `aoa eval exposure scan --out` ledger covering `sample/repo`.
fn write_exposure_scan(base_dir: &Path, repo_id: &str, baseline_commit: &str, status: &str) {
    std::fs::write(
        base_dir.join("exposure.json"),
        format!(
            r#"{{"repos":[{{"repo_id":"{repo_id}","baseline_commit":"{baseline_commit}",
            "total_subjects":1,"status":{status},"provenance":null}}]}}"#
        ),
    )
    .unwrap();
}

fn manifest(repos: &str) -> Manifest {
    let repos: serde_json::Value = serde_json::from_str(&format!("[{repos}]")).unwrap();
    let expected_repo_ids = repos
        .as_array()
        .unwrap()
        .iter()
        .map(|repo| repo["repo_id"].clone())
        .collect::<Vec<_>>();
    serde_json::from_value(serde_json::json!({
        "k_runs": 3,
        "min_holdout_size": 1,
        "expected_repo_ids": expected_repo_ids,
        "repos": repos,
    }))
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
    let manifest: Manifest = serde_json::from_str(
        r#"{"k_runs":3,"min_holdout_size":1,"expected_repo_ids":[],"repos":[]}"#,
    )
    .unwrap();

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
    let edit = repo("", "").replace("sample/repo", "sample/edit");
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

/// A three-seed answer-shaped fixture whose arms agree, so the build reaches the
/// point where a repo's eligibility facts are stated.
fn write_admissible_fixture(base_dir: &Path) -> std::path::PathBuf {
    let tasks_dir = base_dir.join("tasks");
    write_answer_task(&tasks_dir, "alpha");
    // `expected` + `commit` is what makes the oracle externally composed, which
    // every eligibility fact except exposure then satisfies — so these tests turn
    // on the ledger alone.
    std::fs::write(
        tasks_dir.join("alpha/tests/ground_truth.json"),
        format!(
            r#"{{"expected":["src/pkg/app.py"],"answer":["src/pkg/app.py"],
            "answer_type":"file_list","commit":"{BASELINE}"}}"#
        ),
    )
    .unwrap();
    for seed in 1..=3 {
        write_answer_trial(base_dir, &format!("seed{seed}/repo"), "alpha");
        write_answer_trial(base_dir, &format!("seed{seed}/harness"), "alpha");
    }
    std::fs::write(base_dir.join("repo.json"), "{}").unwrap();
    std::fs::write(base_dir.join("harness.json"), "{}").unwrap();
    std::fs::write(
        base_dir.join("calibration.json"),
        r#"{"method":"external_outcome_correlation","protocol_version":"r11","corpus_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","sample_size":20,"criteria":["rho-significant"],"conclusion":"calibrated"}"#,
    )
    .unwrap();
    std::fs::write(
        base_dir.join("index.json"),
        r#"{"documents":[{"relative_path":"src/pkg/app.py","occurrences":[{"symbol":"pkg/app#app().","roles":["definition"]}]}],"aoa":{"writable":[]}}"#,
    )
    .unwrap();
    tasks_dir
}

fn admissible_manifest() -> Manifest {
    let runs = r#"
        {"seed":1,"repo_arm":"seed1/repo","harness_arm":"seed1/harness"},
        {"seed":2,"repo_arm":"seed2/repo","harness_arm":"seed2/harness"},
        {"seed":3,"repo_arm":"seed3/repo","harness_arm":"seed3/harness"}"#;
    manifest(&repo(
        runs,
        r#""task_shape":"answer","scip_index":"index.json","#,
    ))
}

const BASELINE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn an_unexposed_ledger_entry_admits_the_repo() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = write_admissible_fixture(temp.path());
    write_exposure_scan(temp.path(), "sample/repo", BASELINE, r#""unexposed""#);

    let (_, report, _) = build(&admissible_manifest(), &tasks_dir, temp.path()).unwrap();

    assert_eq!(report.repos.len(), 1);
    assert!(report.repos[0].eligible, "got: {:?}", report.repos[0]);
}

#[test]
fn an_exposed_ledger_entry_makes_the_repo_ineligible() {
    // The defect this replaces: the manifest asserted `unexposed` and the gate
    // believed it, so a repo whose held-out subjects were already spent could
    // vote. The verdict now comes from the ledger, which says otherwise.
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = write_admissible_fixture(temp.path());
    write_exposure_scan(temp.path(), "sample/repo", BASELINE, r#""exposed""#);

    let (_, report, _) = build(&admissible_manifest(), &tasks_dir, temp.path()).unwrap();

    assert!(!report.repos[0].eligible, "got: {:?}", report.repos[0]);
    assert!(
        !report.repos[0].exposure.is_unexposed(),
        "the report must publish the measured status, not the manifest's word"
    );
}

#[test]
fn a_ledger_that_does_not_cover_the_repo_fails_the_build() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = write_admissible_fixture(temp.path());
    write_exposure_scan(temp.path(), "other/repo", BASELINE, r#""unexposed""#);

    let error = build(&admissible_manifest(), &tasks_dir, temp.path())
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("has no exposure entry for it"),
        "got: {error}"
    );
    assert!(error.contains("sample/repo"), "got: {error}");
}

#[test]
fn a_ledger_scanned_at_another_revision_fails_the_build() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = write_admissible_fixture(temp.path());
    write_exposure_scan(
        temp.path(),
        "sample/repo",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        r#""unexposed""#,
    );

    let error = build(&admissible_manifest(), &tasks_dir, temp.path())
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("does not describe the revision being measured"),
        "got: {error}"
    );
}

#[test]
fn a_missing_ledger_fails_the_build_rather_than_defaulting_to_unexposed() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = write_admissible_fixture(temp.path());

    let error = build(&admissible_manifest(), &tasks_dir, temp.path())
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("cannot read exposure ledger"),
        "got: {error}"
    );
    assert!(error.contains("exposure.json"), "got: {error}");
}

#[test]
fn an_abbreviated_ledger_revision_still_binds_to_the_manifest_commit() {
    // codeprobe's `prep.json` records an abbreviated `baseline_sha`, so this is
    // the shape a real ledger arrives in.
    let temp = tempfile::tempdir().unwrap();
    let tasks_dir = write_admissible_fixture(temp.path());
    write_exposure_scan(temp.path(), "sample/repo", &BASELINE[..8], r#""exposed""#);

    let (_, report, _) = build(&admissible_manifest(), &tasks_dir, temp.path()).unwrap();

    assert!(!report.repos[0].eligible, "got: {:?}", report.repos[0]);
}
