use super::*;

use aoa_codeprobe_shim::TRACE_DB_SCHEMA;
use rusqlite::{params, Connection};

const CONFIG: &str = "baseline";
const ANSWER_ONLY: &str = "The fix lives in src/lib.py.\n";
const PASSING_SCORING: &str = r#"{"score":1.0,"passed":true}"#;

fn write_trial(run: &Path, task_id: &str, transcript: &str) {
    let trial = run.join(task_id);
    std::fs::create_dir_all(&trial).expect("trial dir");
    std::fs::write(trial.join("agent_output.txt"), transcript).expect("transcript");
    std::fs::write(trial.join("scoring.json"), PASSING_SCORING).expect("scoring");
}

fn trace_db(runs: &Path, rows: &[(&str, &str, i64, &str, &str)]) {
    let conn = Connection::open(runs.join("trace.db")).expect("create trace.db");
    conn.execute_batch("PRAGMA journal_mode=WAL;").expect("wal");
    conn.execute_batch(TRACE_DB_SCHEMA).expect("schema");
    for (run_id, task_id, event_seq, tool_name, tool_input) in rows {
        conn.execute(
            "INSERT INTO events (run_id, config, task_id, event_seq, ts, event_type, tool_name, \
             tool_input, bytes_written) VALUES (?1, ?2, ?3, ?4, 0.0, 'tool_use', ?5, ?6, 0)",
            params![run_id, CONFIG, task_id, event_seq, tool_name, tool_input],
        )
        .expect("insert event");
    }
}

fn answer_only_run(dir: &TempDir) -> PathBuf {
    let run = dir.path().join("runs").join(CONFIG);
    write_trial(&run, "busy-task", ANSWER_ONLY);
    write_trial(&run, "silent-task", ANSWER_ONLY);
    run
}

#[test]
fn eval_run_reads_an_answer_only_trial_from_the_run_trace_db() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    trace_db(
        dir.path().join("runs").as_path(),
        &[
            (
                "run-a",
                "busy-task",
                0,
                "Read",
                r#"{"file_path":"src/lib.py"}"#,
            ),
            (
                "run-a",
                "busy-task",
                1,
                "Read",
                r#"{"file_path":"src/util.py"}"#,
            ),
            (
                "run-a",
                "busy-task",
                2,
                "Edit",
                r#"{"file_path":"src/lib.py","old_string":"a","new_string":"b"}"#,
            ),
            (
                "run-a",
                "silent-task",
                0,
                "Read",
                r#"{"file_path":"README.md"}"#,
            ),
        ],
    );

    let report = eval_run_traces::eval_run_json(&run, &[]);

    assert_eq!(report["record_count"], 2);
    assert_eq!(report["error_count"], 0);
    let busy = eval_run_traces::record(&report, "busy-task");
    assert_eq!(busy["trace_source"], "trace_db");
    assert_eq!(eval_run_traces::count_of(&busy["spans"], "file.read"), 2);
    assert_eq!(
        eval_run_traces::count_of(&busy["spans"], "write.attempt"),
        1,
        "trace.db records no tool results, so the write stays an attempt"
    );
    assert_eq!(
        eval_run_traces::count_of(&busy["spans"], "write.committed"),
        0
    );
    let silent = eval_run_traces::record(&report, "silent-task");
    assert_eq!(silent["trace_source"], "trace_db");
    assert_eq!(eval_run_traces::count_of(&silent["spans"], "abstain"), 1);
}

#[test]
fn eval_run_scores_every_answer_only_trial_of_a_run_from_one_trace_db() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    write_trial(&run, "third-task", ANSWER_ONLY);
    let mut rows = Vec::new();
    for task in ["busy-task", "silent-task", "third-task"] {
        rows.push(("run-a", task, 0, "Read", r#"{"file_path":"src/lib.py"}"#));
        rows.push(("run-a", task, 1, "Grep", r#"{"pattern":"parse"}"#));
    }
    trace_db(dir.path().join("runs").as_path(), &rows);

    let report = eval_run_traces::eval_run_json(&run, &[]);

    assert_eq!(report["record_count"], 3, "{report}");
    assert_eq!(report["error_count"], 0, "{report}");
    for task in ["busy-task", "silent-task", "third-task"] {
        let record = eval_run_traces::record(&report, task);
        assert_eq!(record["trace_source"], "trace_db");
        assert_eq!(eval_run_traces::count_of(&record["spans"], "file.read"), 1);
        assert_eq!(
            eval_run_traces::count_of(&record["spans"], "retrieval.search"),
            1
        );
    }
    assert_eq!(
        std::fs::read_dir(dir.path().join("runs"))
            .expect("list runs")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .count(),
        1,
        "nothing is left beside the run after three trials read the database"
    );
}

#[test]
fn eval_run_human_names_the_trace_source() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    trace_db(
        dir.path().join("runs").as_path(),
        &[
            (
                "run-a",
                "busy-task",
                0,
                "Read",
                r#"{"file_path":"src/lib.py"}"#,
            ),
            (
                "run-a",
                "silent-task",
                0,
                "Read",
                r#"{"file_path":"README.md"}"#,
            ),
        ],
    );

    aoa()
        .args(["eval", "run", "--codeprobe-run"])
        .arg(&run)
        .assert()
        .success()
        .stdout(predicate::str::contains("2 record(s), 0 error(s)"))
        .stdout(predicate::str::contains(") via trace.db"));
}

fn busy_task_db(dir: &TempDir) {
    trace_db(
        dir.path().join("runs").as_path(),
        &[
            (
                "run-a",
                "busy-task",
                0,
                "Read",
                r#"{"file_path":"src/lib.py"}"#,
            ),
            (
                "run-a",
                "silent-task",
                0,
                "Read",
                r#"{"file_path":"README.md"}"#,
            ),
        ],
    );
}

fn eval_run_json_from(cwd: &Path, run_dir: &str) -> Value {
    let output = aoa()
        .current_dir(cwd)
        .args(["eval", "run", "--json", "--codeprobe-run", run_dir])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

#[test]
fn eval_run_finds_the_trace_db_when_the_run_dir_is_spelled_dot() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    busy_task_db(&dir);

    let report = eval_run_json_from(&run, ".");

    assert_eq!(report["error_count"], 0);
    let busy = eval_run_traces::record(&report, "busy-task");
    assert_eq!(busy["trace_source"], "trace_db");
    assert_eq!(eval_run_traces::count_of(&busy["spans"], "file.read"), 1);
}

#[test]
fn eval_run_finds_the_trace_db_when_the_run_dir_is_spelled_dot_dot() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    busy_task_db(&dir);

    let report = eval_run_json_from(&run.join("busy-task"), "..");

    assert_eq!(report["error_count"], 0);
    let busy = eval_run_traces::record(&report, "busy-task");
    assert_eq!(busy["trace_source"], "trace_db");
    assert_eq!(eval_run_traces::count_of(&busy["spans"], "file.read"), 1);
}

#[test]
fn eval_run_keeps_the_transcript_as_source_when_it_carries_events() {
    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("runs").join(CONFIG);
    write_trial(&run, "busy-task", eval_run_traces::READ_AND_EDIT_TRANSCRIPT);
    trace_db(
        dir.path().join("runs").as_path(),
        &[(
            "run-a",
            "busy-task",
            0,
            "Read",
            r#"{"file_path":"ignored.py"}"#,
        )],
    );

    let report = eval_run_traces::eval_run_json(&run, &[]);

    let busy = eval_run_traces::record(&report, "busy-task");
    assert_eq!(busy["trace_source"], "stream_json");
    assert_eq!(eval_run_traces::count_of(&busy["spans"], "file.read"), 2);
    assert_eq!(
        eval_run_traces::count_of(&busy["spans"], "write.committed"),
        1
    );
}

#[test]
fn eval_run_fails_loud_per_trial_on_a_trace_db_it_cannot_attribute() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    trace_db(
        dir.path().join("runs").as_path(),
        &[
            (
                "run-a",
                "busy-task",
                0,
                "Read",
                r#"{"file_path":"src/lib.py"}"#,
            ),
            (
                "run-b",
                "busy-task",
                0,
                "Read",
                r#"{"file_path":"src/lib.py"}"#,
            ),
            (
                "run-a",
                "silent-task",
                0,
                "Read",
                r#"{"file_path":"README.md"}"#,
            ),
        ],
    );

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .output()
        .expect("run");

    assert!(!output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["record_count"], 1);
    assert_eq!(parsed["error_count"], 1);
    assert_eq!(parsed["errors"][0]["task_id"], "busy-task");
    let error = parsed["errors"][0]["error"].as_str().expect("error string");
    for expected in ["trace.db", "run-a, run-b", "busy-task"] {
        assert!(
            error.contains(expected),
            "error must name {expected}: {error}"
        );
    }
}

#[test]
fn eval_run_fails_loud_on_a_trace_db_of_another_schema() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    let conn = Connection::open(dir.path().join("runs").join("trace.db")).expect("create");
    conn.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL NOT NULL); \
         INSERT INTO schema_migrations VALUES (2, 0.0); CREATE TABLE events (run_id TEXT);",
    )
    .expect("foreign schema");
    drop(conn);

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .output()
        .expect("run");

    assert!(!output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["record_count"], 0);
    assert_eq!(parsed["error_count"], 2);
    let error = parsed["errors"][0]["error"].as_str().expect("error string");
    for expected in [
        "differ from the accepted schema",
        "unexpected [(\"table\", \"events\", \"events\", Some(\"CREATE TABLE events(run_id TEXT)\"))]",
    ] {
        assert!(error.contains(expected), "error must name {expected}: {error}");
    }
}

#[test]
fn eval_run_still_rejects_an_answer_only_trial_when_the_trace_db_holds_nothing_for_it() {
    let dir = TempDir::new().expect("tempdir");
    let run = answer_only_run(&dir);
    trace_db(
        dir.path().join("runs").as_path(),
        &[(
            "run-a",
            "busy-task",
            0,
            "Read",
            r#"{"file_path":"src/lib.py"}"#,
        )],
    );

    let output = aoa()
        .args(["eval", "run", "--json", "--codeprobe-run"])
        .arg(&run)
        .output()
        .expect("run");

    assert!(!output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["record_count"], 1);
    assert_eq!(parsed["errors"][0]["task_id"], "silent-task");
    let error = parsed["errors"][0]["error"].as_str().expect("error string");
    assert!(
        error.contains("holds no event") && error.contains("silent-task"),
        "{error}"
    );
}
