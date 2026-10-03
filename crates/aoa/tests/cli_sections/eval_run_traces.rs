use super::*;

const READ_AND_EDIT_TRANSCRIPT: &str = concat!(
    r#"{"type":"system","subtype":"init","tools":["Read","Edit"]}"#,
    "\n",
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"src/lib.py"}}]}}"#,
    "\n",
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"...","is_error":false}]}}"#,
    "\n",
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"src/util.py"}}]}}"#,
    "\n",
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"...","is_error":false}]}}"#,
    "\n",
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t3","name":"Edit","input":{"file_path":"src/lib.py","old_string":"a","new_string":"b"}}]}}"#,
    "\n",
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":"edit applied","is_error":false}]}}"#,
    "\n",
    r#"{"type":"result","subtype":"success","is_error":false}"#,
    "\n",
);

const PROSE_ONLY_TRANSCRIPT: &str = concat!(
    r#"{"type":"system","subtype":"init","tools":[]}"#,
    "\n",
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
    "\n",
    r#"{"type":"result","subtype":"success","is_error":false}"#,
    "\n",
);

const NO_AGENT_EVENTS_TRANSCRIPT: &str = concat!(
    r#"{"type":"system","subtype":"init","tools":[]}"#,
    "\n",
    r#"{"type":"result","subtype":"success","is_error":false}"#,
    "\n",
);

const PASSING_SCORING: &str = r#"{"score":1.0,"passed":true}"#;

fn write_trial(run: &Path, task_id: &str, transcript: &str, scoring: Option<&str>) {
    let trial = run.join(task_id);
    std::fs::create_dir_all(&trial).expect("trial dir");
    std::fs::write(trial.join("agent_output.txt"), transcript).expect("transcript");
    if let Some(scoring) = scoring {
        std::fs::write(trial.join("scoring.json"), scoring).expect("scoring");
    }
}

fn two_trial_run(dir: &TempDir) -> PathBuf {
    let run = dir.path().join("run");
    write_trial(
        &run,
        "busy-task",
        READ_AND_EDIT_TRANSCRIPT,
        Some(PASSING_SCORING),
    );
    write_trial(
        &run,
        "silent-task",
        PROSE_ONLY_TRANSCRIPT,
        Some(PASSING_SCORING),
    );
    run
}

fn record<'a>(report: &'a Value, task_id: &str) -> &'a Value {
    report["records"]
        .as_array()
        .expect("records array")
        .iter()
        .find(|r| r["task_id"] == task_id)
        .unwrap_or_else(|| panic!("no record for {task_id}"))
}

fn count_of(spans: &Value, span_type: &str) -> u64 {
    spans["counts"]
        .as_array()
        .expect("counts array")
        .iter()
        .find(|c| c["span_type"] == span_type)
        .map_or(0, |c| c["count"].as_u64().expect("count"))
}

fn eval_run_json(run: &Path, extra: &[&std::ffi::OsStr]) -> Value {
    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(run)
        .args(extra)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

fn validate_trace_json(path: &Path) -> Value {
    let output = aoa()
        .args(["eval", "validate-trace", "--json"])
        .arg(path)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{} must pass validate-trace: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

#[test]
fn eval_run_json_reports_span_counts_by_type_per_task() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);

    let report = eval_run_json(&run, &[]);

    let busy = &record(&report, "busy-task")["spans"];
    assert_eq!(count_of(busy, "file.read"), 2);
    assert_eq!(count_of(busy, "write.committed"), 1);
    let summed: u64 = busy["counts"]
        .as_array()
        .expect("counts")
        .iter()
        .map(|c| c["count"].as_u64().expect("count"))
        .sum();
    assert_eq!(busy["total"].as_u64(), Some(summed));
    assert!(report.get("traces_emitted").is_none());
}

#[test]
fn a_task_whose_agent_called_no_tools_reports_only_the_abstain_span() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);

    let report = eval_run_json(&run, &[]);

    let silent = &record(&report, "silent-task")["spans"];
    assert_eq!(silent["total"], 1);
    assert_eq!(
        silent["counts"],
        serde_json::json!([{"span_type": "abstain", "count": 1}])
    );
}

#[test]
fn eval_run_human_register_reports_span_counts_per_task() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);

    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(&run)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "spans: 3 (file.read 2, write.committed 1)",
        ))
        .stdout(predicate::str::contains("spans: 1 (abstain 1)"));
}

#[test]
fn a_transcript_with_no_agent_events_is_an_error_and_gets_no_trace_file() {
    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("run");
    write_trial(
        &run,
        "empty-task",
        NO_AGENT_EVENTS_TRANSCRIPT,
        Some(PASSING_SCORING),
    );
    let traces = dir.path().join("traces");

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .arg("--emit-traces")
        .arg(&traces)
        .output()
        .expect("run");

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(report["record_count"], 0);
    assert_eq!(report["errors"][0]["task_id"], "empty-task");
    assert_eq!(report["traces_emitted"]["count"], 0);
    assert!(!traces.join("empty-task.trace.json").exists());
}

#[test]
fn emitted_traces_pass_validate_trace_and_match_the_reported_counts() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);
    let traces = dir.path().join("out").join("traces");

    let report = eval_run_json(&run, &["--emit-traces".as_ref(), traces.as_os_str()]);

    assert_eq!(report["traces_emitted"]["count"], 2);
    for task_id in ["busy-task", "silent-task"] {
        let validated = validate_trace_json(&traces.join(format!("{task_id}.trace.json")));
        let reported = &record(&report, task_id)["spans"];
        assert_eq!(&validated, reported, "{task_id}");
    }
}

#[test]
fn human_register_names_where_traces_were_written() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);
    let traces = dir.path().join("traces");

    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(&run)
        .arg("--emit-traces")
        .arg(&traces)
        .assert()
        .success()
        .stdout(predicate::str::contains("traces: 2 file(s) written to"));
}

#[test]
fn a_trial_that_fails_after_its_trace_was_reconstructed_still_gets_a_trace_file() {
    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("run");
    write_trial(&run, "unscored-task", READ_AND_EDIT_TRANSCRIPT, None);
    let traces = dir.path().join("traces");

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .arg("--emit-traces")
        .arg(&traces)
        .output()
        .expect("run");

    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(report["error_count"], 1);
    assert_eq!(report["traces_emitted"]["count"], 1);
    let validated = validate_trace_json(&traces.join("unscored-task.trace.json"));
    assert_eq!(count_of(&validated, "file.read"), 2);
}

#[test]
fn without_the_flag_no_trace_directory_is_created() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);

    eval_run_json(&run, &[]);

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read tempdir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("run")]);
}

#[cfg(unix)]
#[test]
fn emitting_over_a_planted_symlink_leaves_its_target_untouched() {
    let dir = TempDir::new().expect("tempdir");
    let run = two_trial_run(&dir);
    let traces = dir.path().join("traces");
    std::fs::create_dir_all(&traces).expect("traces dir");
    let planted = dir.path().join("planted.txt");
    std::fs::write(&planted, "do not touch").expect("planted target");
    let destination = traces.join("busy-task.trace.json");
    std::os::unix::fs::symlink(&planted, &destination).expect("symlink");

    eval_run_json(&run, &["--emit-traces".as_ref(), traces.as_os_str()]);

    assert_eq!(
        std::fs::read_to_string(&planted).expect("planted target"),
        "do not touch"
    );
    assert!(!std::fs::symlink_metadata(&destination)
        .expect("destination")
        .file_type()
        .is_symlink());
    validate_trace_json(&destination);
}
