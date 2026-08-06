use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use aoa_bench::scan_exposure;
use aoa_gap::ExposureStatus;
use serde_json::json;
use tempfile::TempDir;

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn instruction(repo: &str, family: &str, subject: &str) -> String {
    format!(
        "# {family}: {subject}\n\n**Repository:** {repo}\n\n**Task type:** architecture_comprehension\n"
    )
}

fn campaign_fixture(current: usize, exposed: usize) -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let runs = temp.path().join("runs");
    let repo = temp.path().join("repo");
    let campaign_repo = runs.join("httpie");
    let task_ids: Vec<_> = (0..current).map(|i| format!("current-{i}")).collect();

    write(
        &campaign_repo.join("prep.json"),
        serde_json::to_vec(&json!({
            "repo": "httpie",
            "baseline_path": repo,
            "baseline_sha": "5b604c37"
        }))
        .unwrap(),
    );
    write(
        &campaign_repo.join("mine.json"),
        serde_json::to_vec(&json!({
            "repo": "httpie",
            "task_ids": task_ids
        }))
        .unwrap(),
    );

    for i in 0..current {
        write(
            &repo.join(format!(".codeprobe/tasks/current-{i}/instruction.md")),
            instruction(
                "httpie",
                "dependency_analysis",
                &format!("httpie.subject.{i}"),
            ),
        );
    }
    for i in 0..exposed {
        let trial = runs.join(format!("quarantine/old-{i}"));
        write(&trial.join("scoring.json"), "{}");
        write(
            &trial.join("instruction.resolved.md"),
            instruction(
                "httpie",
                "dependency_analysis",
                &format!("httpie.subject.{i}"),
            ),
        );
    }

    (temp, runs)
}

#[test]
fn renamed_trials_rediscover_exactly_seven_of_fourteen_subjects() {
    let (_temp, runs) = campaign_fixture(14, 7);

    let scan = scan_exposure(&runs).unwrap();
    let repo = scan
        .repos
        .iter()
        .find(|repo| repo.repo_id == "httpie")
        .unwrap();

    assert_eq!(repo.total_subjects, 14);
    assert_eq!(repo.exposed_subject_count(), 7);
    assert!(matches!(
        repo.status,
        ExposureStatus::PartiallyExposed { .. }
    ));
    let provenance = repo.provenance.as_ref().unwrap();
    assert_eq!(provenance.trial_count, 7);
    assert_eq!(provenance.unscored_trials, 7);
}

#[test]
fn void_scored_trials_remain_exposed_and_report_their_provenance() {
    let (_temp, runs) = campaign_fixture(2, 2);
    for index in 0..2 {
        write(
            &runs.join(format!("quarantine/old-{index}/scoring.json")),
            r#"{"score":0.0}"#,
        );
    }

    let scan = scan_exposure(&runs).unwrap();
    let repo = &scan.repos[0];

    assert_eq!(repo.status, ExposureStatus::Exposed);
    let provenance = repo
        .provenance
        .as_ref()
        .expect("persisted exposure carries provenance");
    assert_eq!(
        provenance.causing_run_paths,
        [runs.join("quarantine")].into_iter().collect()
    );
    assert_eq!(provenance.trial_count, 2);
    assert!(provenance.mtime_range.earliest_unix_ms <= provenance.mtime_range.latest_unix_ms);
    assert_eq!(provenance.held_out_passed, 0);
    assert_eq!(provenance.held_out_failed, 2);
    assert_eq!(provenance.unscored_trials, 0);
}

#[test]
fn real_r0_campaign_matches_documented_exposure_and_held_out_provenance() {
    const RUNS_ROOT: &str = "/home/ds/projects/codeprobe/runs/r0-campaign";
    const HTTPIE_BASELINE: &str = "5b604c37c6c67e18e7c3e9aee6c88a8c22b98345";

    let scan = scan_exposure(Path::new(RUNS_ROOT)).unwrap();
    let repo = scan
        .repos
        .iter()
        .find(|repo| repo.repo_id == "httpie")
        .unwrap();

    assert_eq!(repo.baseline_commit, HTTPIE_BASELINE);
    assert_eq!(repo.total_subjects, 14);
    let ExposureStatus::PartiallyExposed { subjects } = &repo.status else {
        panic!(
            "expected HTTPie to be partially exposed, got {:?}",
            repo.status
        );
    };
    let expected: BTreeSet<_> = [
        ("dependency_analysis", "httpie.__main__.main"),
        ("dependency_analysis", "httpie.cli.nested_json.parse.parse"),
        ("dependency_analysis", "httpie.compat.func"),
        ("dependency_analysis", "httpie.core.program"),
        ("dependency_analysis", "httpie.manager.core.program"),
        ("import_chain", "httpie.cookies"),
        ("import_chain", "httpie.compat"),
    ]
    .into_iter()
    .map(
        |(question_family, oracle_target_symbol)| aoa_gap::SubjectKey {
            repo_id: "httpie".to_string(),
            baseline_commit: HTTPIE_BASELINE.to_string(),
            oracle_target_symbol: oracle_target_symbol.to_string(),
            question_family: question_family.to_string(),
        },
    )
    .collect();

    assert_eq!(subjects, &expected);

    // Every one of these trials reports a top-level score of 0.0 while its
    // independent artifact leg passed — the composite was lowered by the
    // gameable direct leg. The ledger reports the held-out leg, so these read
    // as passes, not as the "0.0" the old private scoring copy counted.
    for (repo_id, expected_trials, expected_run_paths, expected_passed) in
        [("sqlparse", 56, 4, 56), ("websockets", 24, 2, 23)]
    {
        let repo = scan
            .repos
            .iter()
            .find(|repo| repo.repo_id == repo_id)
            .unwrap();
        assert_eq!(repo.status, ExposureStatus::Exposed);
        let provenance = repo.provenance.as_ref().unwrap();
        assert_eq!(provenance.trial_count, expected_trials);
        assert_eq!(provenance.causing_run_paths.len(), expected_run_paths);
        assert_eq!(provenance.held_out_passed, expected_passed);
        assert_eq!(
            provenance.held_out_failed,
            expected_trials - expected_passed
        );
        assert_eq!(provenance.errored_trials, 0);
        assert_eq!(provenance.unscored_trials, 0);
    }
}

#[test]
fn spent_trial_without_subject_identity_fails_loud() {
    let (_temp, runs) = campaign_fixture(1, 0);
    write(&runs.join("quarantine/orphan/scoring.json"), "{}");

    let error = scan_exposure(&runs).unwrap_err();

    assert!(error.to_string().contains("instruction"), "got: {error}");
}

#[test]
fn empty_tree_is_not_reported_as_an_all_clear() {
    let temp = tempfile::tempdir().unwrap();

    let error = scan_exposure(temp.path()).unwrap_err();

    assert!(error.to_string().contains("no campaign"), "got: {error}");
}

#[test]
fn task_id_cannot_escape_the_pinned_task_directory() {
    let (temp, runs) = campaign_fixture(1, 0);
    let campaign_repo = runs.join("httpie");
    write(
        &campaign_repo.join("mine.json"),
        r#"{"repo":"httpie","task_ids":["../outside"]}"#,
    );
    write(
        &temp.path().join("repo/.codeprobe/outside/instruction.md"),
        instruction("httpie", "dependency_analysis", "escaped.subject"),
    );

    let error = scan_exposure(&runs).unwrap_err();

    assert!(error.to_string().contains("task id"), "got: {error}");
}

#[test]
fn prep_without_mine_manifest_fails_the_whole_scan() {
    let (_temp, runs) = campaign_fixture(1, 0);
    write(
        &runs.join("orphan/prep.json"),
        r#"{"repo":"orphan","baseline_path":"/unused","baseline_sha":"abc"}"#,
    );

    let error = scan_exposure(&runs).unwrap_err();

    assert!(error.to_string().contains("mine.json"), "got: {error}");
}

#[test]
fn mine_without_prep_manifest_fails_the_whole_scan() {
    let (_temp, runs) = campaign_fixture(1, 0);
    write(
        &runs.join("orphan/mine.json"),
        r#"{"repo":"orphan","task_ids":["task"]}"#,
    );

    let error = scan_exposure(&runs).unwrap_err();

    assert!(error.to_string().contains("prep.json"), "got: {error}");
}

#[test]
fn empty_admitted_corpus_is_not_unexposed() {
    let (_temp, runs) = campaign_fixture(1, 0);
    write(
        &runs.join("httpie/mine.json"),
        r#"{"repo":"httpie","task_ids":[]}"#,
    );

    let error = scan_exposure(&runs).unwrap_err();

    assert!(
        error.to_string().contains("no admitted tasks"),
        "got: {error}"
    );
}

#[cfg(unix)]
#[test]
fn scanner_does_not_follow_symlinked_quarantine_directories() {
    use std::os::unix::fs::symlink;

    let (_temp, runs) = campaign_fixture(1, 0);
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("trial/scoring.json"), "{}");
    write(
        &outside.path().join("trial/instruction.resolved.md"),
        instruction("httpie", "dependency_analysis", "httpie.subject.0"),
    );
    symlink(outside.path(), runs.join("linked-quarantine")).unwrap();

    let scan = scan_exposure(&runs).unwrap();

    assert_eq!(scan.repos[0].status, ExposureStatus::Unexposed);
}

#[test]
fn dual_composite_trial_is_counted_by_its_held_out_leg_not_its_composite() {
    let (_temp, runs) = campaign_fixture(2, 2);
    // The shape the private ExposureScoring copy got wrong: the artifact
    // (held-out) leg passed, and only the gameable direct leg failed, so the
    // top-level composite was lowered to 0.0. The ledger tracks held-out.
    for index in 0..2 {
        write(
            &runs.join(format!("quarantine/old-{index}/scoring.json")),
            r#"{"score":0.0,"passed":false,"scorer_family":"dual_composite",
                "passed_direct":false,"passed_artifact":true,
                "score_direct":0.0,"score_artifact":1.0}"#,
        );
    }

    let scan = scan_exposure(&runs).unwrap();
    let provenance = scan.repos[0].provenance.as_ref().unwrap();

    assert_eq!(provenance.trial_count, 2);
    assert_eq!(provenance.held_out_passed, 2);
    assert_eq!(provenance.held_out_failed, 0);
    assert_eq!(provenance.unscored_trials, 0);
}

#[test]
fn persisted_scorer_error_is_its_own_category_not_folded_into_unscored() {
    let (_temp, runs) = campaign_fixture(2, 2);
    write(
        &runs.join("quarantine/old-0/scoring.json"),
        r#"{"error":"scorer crashed on malformed answer.json"}"#,
    );

    let scan = scan_exposure(&runs).unwrap();
    let provenance = scan.repos[0].provenance.as_ref().unwrap();

    assert_eq!(scan.repos[0].status, ExposureStatus::Exposed);
    assert_eq!(provenance.trial_count, 2);
    assert_eq!(provenance.errored_trials, 1);
    assert_eq!(provenance.unscored_trials, 1);
    assert_eq!(provenance.held_out_passed, 0);
    assert_eq!(provenance.held_out_failed, 0);
}

#[test]
fn dual_leg_error_without_a_top_level_error_is_tallied_not_fatal() {
    let (_temp, runs) = campaign_fixture(2, 2);
    // A dual_composite scorer records a failed held-out leg in `error_artifact`
    // and leaves the top-level `error` null, so a check that read only the top
    // level would let held_out_outcome() raise and abort the whole scan.
    write(
        &runs.join("quarantine/old-0/scoring.json"),
        r#"{"score":0.0,"error":null,"scorer_family":"dual_composite",
            "passed_direct":true,"score_direct":1.0,
            "error_artifact":"answer.json not found"}"#,
    );

    let scan = scan_exposure(&runs).unwrap();
    let provenance = scan.repos[0].provenance.as_ref().unwrap();

    assert_eq!(scan.repos[0].status, ExposureStatus::Exposed);
    assert_eq!(provenance.errored_trials, 1);
    assert_eq!(provenance.unscored_trials, 1);
}
