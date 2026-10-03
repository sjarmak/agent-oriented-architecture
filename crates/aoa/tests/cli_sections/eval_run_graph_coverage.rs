use super::eval_run::{run_dir, tasks_dir};
use super::*;

fn repo_with(typescript_files: usize, python_files: usize) -> TempDir {
    let repo = TempDir::new().expect("temp repo");
    std::fs::create_dir_all(repo.path().join("src")).expect("src dir");
    std::fs::create_dir_all(repo.path().join("tools")).expect("tools dir");
    for i in 0..typescript_files {
        std::fs::write(
            repo.path().join(format!("src/module{i}.ts")),
            format!("export function handler{i}(): number {{ return {i}; }}\n"),
        )
        .expect("write ts");
    }
    for i in 0..python_files {
        std::fs::write(
            repo.path().join(format!("tools/x{i}.py")),
            format!("def helper{i}():\n    return {i}\n"),
        )
        .expect("write py");
    }
    repo
}

fn eval_run_json(repo: &Path) -> Value {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .arg("--repo")
        .arg(repo)
        .output()
        .expect("run");
    serde_json::from_slice(&output.stdout).expect("valid json")
}

#[test]
fn eval_run_gives_no_weight_to_a_graph_that_missed_the_main_language() {
    let repo = repo_with(100, 1);
    let parsed = eval_run_json(repo.path());

    let records = parsed["records"].as_array().expect("records");
    assert!(!records.is_empty());
    for rec in records {
        assert_eq!(rec["graph_quality"], "degraded");
        assert_eq!(rec["weight"], 0.0);
        assert_eq!(rec["repo_eligible_for_r0"], false);
        let reason = rec["graph_degrade_reason"].as_str().expect("reason");
        assert!(reason.contains("TypeScript"), "got: {reason}");
        assert!(reason.contains("--scip-index"), "got: {reason}");
        assert!(
            rec["mutation_surface"]["reachable"]
                .as_array()
                .expect("reachable")
                .is_empty(),
            "no surface may be computed over the incidental Python"
        );
    }
    assert_eq!(parsed["graph_coverage"]["indexed"]["Python"], 1);
    assert_eq!(parsed["graph_coverage"]["unindexed"]["TypeScript"], 100);
}

#[test]
fn eval_run_names_the_unindexed_language_when_no_file_was_indexable() {
    let repo = repo_with(5, 0);
    let parsed = eval_run_json(repo.path());

    let rec = &parsed["records"][0];
    assert_eq!(rec["graph_quality"], "degraded");
    let reason = rec["graph_degrade_reason"].as_str().expect("reason");
    assert!(reason.contains("TypeScript"), "got: {reason}");
}

#[test]
fn eval_run_human_register_names_the_unindexed_language() {
    let repo = repo_with(100, 1);
    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(run_dir())
        .arg("--tasks")
        .arg(tasks_dir())
        .arg("--repo")
        .arg(repo.path())
        .assert()
        .stdout(predicate::str::contains("weight=0.0 graph=degraded"))
        .stdout(predicate::str::contains("graph coverage: Python 1 of 101"))
        .stdout(predicate::str::contains("TypeScript (100 files)"))
        .stderr(predicate::str::contains("TypeScript"));
}

#[test]
fn eval_run_keeps_a_mixed_repo_weighted_and_reports_what_it_skipped() {
    let repo = repo_with(3, 2);
    let parsed = eval_run_json(repo.path());

    let rec = &parsed["records"][0];
    assert_eq!(rec["graph_quality"], "best_effort");
    assert_eq!(rec["weight"], 0.5);
    assert!(rec.get("graph_degrade_reason").is_none());
    assert_eq!(parsed["graph_coverage"]["indexed"]["Python"], 2);
    assert_eq!(parsed["graph_coverage"]["unindexed"]["TypeScript"], 3);
}

#[test]
fn eval_run_reports_full_coverage_for_a_python_only_repo() {
    let repo = repo_with(0, 2);
    let parsed = eval_run_json(repo.path());

    assert_eq!(parsed["records"][0]["graph_quality"], "best_effort");
    assert_eq!(parsed["graph_coverage"]["indexed"]["Python"], 2);
    assert_eq!(parsed["graph_coverage"]["unindexed"], serde_json::json!({}));
}
