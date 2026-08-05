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
