use super::*;

#[test]
fn exposure_scan_json_reports_subject_keyed_partial_status() {
    let temp = tempfile::tempdir().unwrap();
    let runs = temp.path().join("runs");
    let repo = temp.path().join("repo");
    let campaign_repo = runs.join("httpie");
    std::fs::create_dir_all(&campaign_repo).unwrap();
    std::fs::write(
        campaign_repo.join("prep.json"),
        serde_json::to_vec(&serde_json::json!({
            "repo": "httpie",
            "baseline_path": repo,
            "baseline_sha": "abc123"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        campaign_repo.join("mine.json"),
        r#"{"repo":"httpie","task_ids":["current-a","current-b"]}"#,
    )
    .unwrap();
    let current_a = repo.join(".codeprobe/tasks/current-a");
    let current_b = repo.join(".codeprobe/tasks/current-b");
    std::fs::create_dir_all(&current_a).unwrap();
    std::fs::create_dir_all(&current_b).unwrap();
    std::fs::write(
        current_a.join("instruction.md"),
        "# import_chain: httpie.cookies\n\n**Repository:** httpie\n",
    )
    .unwrap();
    std::fs::write(
        current_b.join("instruction.md"),
        "# import_chain: httpie.utils\n\n**Repository:** httpie\n",
    )
    .unwrap();
    let spent = runs.join("quarantine/prior");
    std::fs::create_dir_all(&spent).unwrap();
    std::fs::write(spent.join("agent_output.txt"), "transcript").unwrap();
    std::fs::write(
        spent.join("instruction.resolved.md"),
        "# import_chain: httpie.cookies\n\n**Repository:** httpie\n",
    )
    .unwrap();

    let output = aoa()
        .args(["eval", "exposure", "scan", "--runs"])
        .arg(&runs)
        .arg("--json")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["repos"][0]["repo_id"], "httpie");
    let subjects = body["repos"][0]["status"]["partially_exposed"]["subjects"]
        .as_array()
        .expect("partial status carries exact subjects");
    assert_eq!(subjects.len(), 1);
    assert_eq!(subjects[0]["oracle_target_symbol"], "httpie.cookies");
}
