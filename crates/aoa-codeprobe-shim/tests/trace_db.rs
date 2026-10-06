//! Integration tests for the `trace.db` source: a fixture database built by
//! each test from the DDL the reader states, read back through the public
//! surface, and never modified by the read.

use std::path::{Path, PathBuf};

use aoa_codeprobe_shim::{
    parse_trace_db, parse_trial, ShimError, TraceDbError, TraceDbTrial, TraceSource,
    TRACE_DB_SCHEMA,
};
use aoa_trace::{validate_trace_value, SpanSource, SpanType};
use rusqlite::{params, Connection};
use tempfile::TempDir;

const CONFIG: &str = "baseline";
const TASK: &str = "task-001";

struct Row<'a> {
    run_id: &'a str,
    config: &'a str,
    task_id: &'a str,
    event_seq: i64,
    event_type: &'a str,
    tool_name: Option<&'a str>,
    tool_input: Option<&'a str>,
}

fn tool_use(event_seq: i64, tool_name: &'static str, tool_input: &'static str) -> Row<'static> {
    Row {
        run_id: "run-a",
        config: CONFIG,
        task_id: TASK,
        event_seq,
        event_type: "tool_use",
        tool_name: Some(tool_name),
        tool_input: Some(tool_input),
    }
}

fn marker(event_seq: i64, event_type: &'static str) -> Row<'static> {
    Row {
        run_id: "run-a",
        config: CONFIG,
        task_id: TASK,
        event_seq,
        event_type,
        tool_name: None,
        tool_input: None,
    }
}

fn open_fixture(dir: &TempDir) -> (PathBuf, Connection) {
    let path = dir.path().join("trace.db");
    let conn = Connection::open(&path).expect("create fixture database");
    conn.execute_batch("PRAGMA journal_mode=WAL;").expect("wal");
    conn.execute_batch(TRACE_DB_SCHEMA).expect("schema");
    (path, conn)
}

fn insert(conn: &Connection, row: &Row<'_>) {
    conn.execute(
        "INSERT INTO events (run_id, config, task_id, event_seq, ts, event_type, tool_name, \
         tool_input, tool_output, bytes_written) VALUES (?1, ?2, ?3, ?4, 1.5, ?5, ?6, ?7, NULL, 0)",
        params![
            row.run_id,
            row.config,
            row.task_id,
            row.event_seq,
            row.event_type,
            row.tool_name,
            row.tool_input
        ],
    )
    .expect("insert event");
}

fn fixture(dir: &TempDir, rows: &[Row<'_>]) -> TraceDbTrial {
    let (path, conn) = open_fixture(dir);
    for row in rows {
        insert(&conn, row);
    }
    drop(conn);
    trial(path)
}

fn trace_db_err(err: ShimError) -> TraceDbError {
    match err {
        ShimError::TraceDb(inner) => *inner,
        other => panic!("expected a trace.db failure, got {other}"),
    }
}

fn trial(path: PathBuf) -> TraceDbTrial {
    TraceDbTrial {
        path,
        config: CONFIG.to_string(),
        task_id: TASK.to_string(),
    }
}

fn busy_rows() -> Vec<Row<'static>> {
    vec![
        tool_use(0, "Read", r#"{"file_path":"src/lib.py"}"#),
        tool_use(1, "Grep", r#"{"pattern":"def parse","path":"src"}"#),
        tool_use(2, "Bash", r#"{"command":"ls"}"#),
        tool_use(3, "mcp__graph__lookup", r#"{"symbol":"parse"}"#),
        tool_use(
            4,
            "Edit",
            r#"{"file_path":"src/lib.py","old_string":"a","new_string":"b"}"#,
        ),
        tool_use(5, "Bash", r#"{"command":"pytest -q"}"#),
        marker(6, "result"),
    ]
}

fn target<'a>(span: &'a aoa_trace::Span, key: &str) -> &'a str {
    span.attributes[key].as_str().expect("string target")
}

#[test]
fn reads_a_trial_in_event_order_as_a_valid_native_trace() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());

    let result = parse_trace_db(&trial).expect("fixture parses");
    validate_trace_value(&result.trace).expect("trace validates");

    let spans = &result.trace.spans;
    let types: Vec<SpanType> = spans.iter().map(|s| s.span_type).collect();
    assert_eq!(
        types,
        vec![
            SpanType::FileRead,
            SpanType::RetrievalSearch,
            SpanType::GatewayInvoke,
            SpanType::WriteAttempt,
            SpanType::TestRun,
        ]
    );
    assert_eq!(target(&spans[0], "path"), "src/lib.py");
    assert_eq!(target(&spans[1], "query"), "def parse");
    assert_eq!(target(&spans[2], "tool"), "mcp__graph__lookup");
    assert_eq!(target(&spans[3], "path"), "src/lib.py");
    assert_eq!(target(&spans[4], "command"), "pytest -q");
    assert!(spans.iter().all(|s| s.source == SpanSource::Native));
    let seqs: Vec<u64> = spans.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4]);
    assert_eq!(result.warnings.len(), 1);
    assert!(
        result.warnings[0].contains("unmapped tool 'Bash'"),
        "{:?}",
        result.warnings
    );
}

#[test]
fn a_write_stays_an_attempt_because_the_database_records_no_outcome() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());

    let result = parse_trace_db(&trial).expect("fixture parses");

    assert!(result
        .trace
        .spans
        .iter()
        .all(|s| s.span_type != SpanType::WriteCommitted && s.span_type != SpanType::WriteBlocked));
    assert!(result
        .trace
        .spans
        .iter()
        .any(|s| s.span_type == SpanType::WriteAttempt));
}

#[test]
fn a_trial_without_a_write_gets_a_trailing_abstain_span() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(
        &dir,
        &[
            tool_use(0, "Read", r#"{"file_path":"src/lib.py"}"#),
            marker(1, "result"),
        ],
    );

    let result = parse_trace_db(&trial).expect("fixture parses");

    let types: Vec<SpanType> = result.trace.spans.iter().map(|s| s.span_type).collect();
    assert_eq!(types, vec![SpanType::FileRead, SpanType::Abstain]);
}

#[test]
fn rows_are_selected_by_config_and_task_and_ordered_by_event_seq() {
    let dir = TempDir::new().expect("tempdir");
    let other_config = Row {
        config: "other-config",
        ..tool_use(0, "Edit", r#"{"file_path":"elsewhere.py"}"#)
    };
    let other_task = Row {
        task_id: "task-002",
        ..tool_use(0, "Edit", r#"{"file_path":"elsewhere.py"}"#)
    };
    let trial = fixture(
        &dir,
        &[
            tool_use(7, "Read", r#"{"file_path":"second.py"}"#),
            tool_use(3, "Read", r#"{"file_path":"first.py"}"#),
            other_config,
            other_task,
        ],
    );

    let result = parse_trace_db(&trial).expect("fixture parses");

    let paths: Vec<&str> = result
        .trace
        .spans
        .iter()
        .filter(|s| s.span_type == SpanType::FileRead)
        .map(|s| target(s, "path"))
        .collect();
    assert_eq!(paths, vec!["first.py", "second.py"]);
    assert!(result
        .trace
        .spans
        .iter()
        .all(|s| s.span_type != SpanType::WriteAttempt));
}

#[test]
fn events_still_in_the_write_ahead_log_are_read() {
    let dir = TempDir::new().expect("tempdir");
    let (path, writer) = open_fixture(&dir);
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .expect("keep frames in the wal");
    insert(
        &writer,
        &tool_use(0, "Read", r#"{"file_path":"src/lib.py"}"#),
    );
    assert!(
        std::fs::metadata(dir.path().join("trace.db-wal"))
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        "the fixture must still hold its rows in the wal"
    );

    let result = parse_trace_db(&trial(path)).expect("rows in the wal are read");

    assert_eq!(result.trace.spans[0].span_type, SpanType::FileRead);
    drop(writer);
}

#[test]
fn the_read_leaves_the_database_bytes_untouched() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());
    let before = std::fs::read(&trial.path).expect("read fixture bytes");

    parse_trace_db(&trial).expect("fixture parses");
    let malformed = fixture(
        &TempDir::new().expect("tempdir"),
        &[tool_use(0, "Read", "not json")],
    );
    parse_trace_db(&malformed).expect_err("malformed input fails");

    assert_eq!(std::fs::read(&trial.path).expect("re-read"), before);
}

#[test]
fn a_database_holding_no_event_for_the_trial_fails_loud() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(
        &dir,
        &[Row {
            task_id: "task-002",
            ..tool_use(0, "Read", r#"{"file_path":"a"}"#)
        }],
    );

    let err = trace_db_err(parse_trace_db(&trial).unwrap_err());

    assert!(
        matches!(&err, TraceDbError::NoEvents { config, task_id, .. }
            if config == CONFIG && task_id == TASK),
        "{err}"
    );
}

#[test]
fn a_trial_written_by_two_runs_is_ambiguous() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(
        &dir,
        &[
            tool_use(0, "Read", r#"{"file_path":"a"}"#),
            Row {
                run_id: "run-b",
                ..tool_use(0, "Read", r#"{"file_path":"b"}"#)
            },
        ],
    );

    let err = trace_db_err(parse_trace_db(&trial).unwrap_err());

    assert!(
        matches!(&err, TraceDbError::AmbiguousRun { runs, .. }
            if runs == &["run-a".to_string(), "run-b".to_string()]),
        "{err}"
    );
    assert!(err.to_string().contains("run-a, run-b"), "{err}");
}

#[test]
fn a_truncated_trial_is_refused_rather_than_scored_short() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(
        &dir,
        &[
            tool_use(0, "Read", r#"{"file_path":"a"}"#),
            marker(1, "trace_truncated"),
        ],
    );

    let err = trace_db_err(parse_trace_db(&trial).unwrap_err());

    assert!(
        matches!(err, TraceDbError::Truncated { event_seq: 1, .. }),
        "{err}"
    );
}

#[test]
fn malformed_events_fail_loud_naming_the_event() {
    let cases: [(&str, Row<'static>); 4] = [
        ("unknown event_type", marker(0, "tool_result")),
        ("tool_use row has no tool_name", marker(0, "tool_use")),
        (
            "tool_use row has no tool_input",
            Row {
                tool_input: None,
                ..tool_use(0, "Read", "")
            },
        ),
        ("tool_input is not JSON", tool_use(0, "Read", "{not json")),
    ];
    for (expected, row) in cases {
        let dir = TempDir::new().expect("tempdir");
        let trial = fixture(&dir, &[row]);

        let err = trace_db_err(parse_trace_db(&trial).unwrap_err());

        assert!(
            matches!(&err, TraceDbError::MalformedEvent { event_seq: 0, detail, .. }
                if detail.contains(expected)),
            "{expected}: {err}"
        );
    }
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &[tool_use(0, "Read", "[1, 2]")]);
    let err = trace_db_err(parse_trace_db(&trial).unwrap_err());
    assert!(
        matches!(&err, TraceDbError::MalformedEvent { detail, .. }
            if detail.contains("not a JSON object")),
        "{err}"
    );
}

#[test]
fn another_schema_version_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let (path, conn) = open_fixture(&dir);
    conn.execute_batch("INSERT INTO schema_migrations VALUES (2, 0.0);")
        .expect("bump");
    insert(&conn, &tool_use(0, "Read", r#"{"file_path":"a"}"#));
    drop(conn);

    let err = trace_db_err(parse_trace_db(&trial(path)).unwrap_err());

    assert!(
        matches!(&err, TraceDbError::Schema { detail, .. } if detail.contains("[1, 2]")),
        "{err}"
    );
}

#[test]
fn an_events_table_with_other_columns_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let (path, conn) = open_fixture(&dir);
    conn.execute_batch("ALTER TABLE events ADD COLUMN is_error INTEGER;")
        .expect("alter");
    insert(&conn, &tool_use(0, "Read", r#"{"file_path":"a"}"#));
    drop(conn);

    let err = trace_db_err(parse_trace_db(&trial(path)).unwrap_err());

    assert!(
        matches!(&err, TraceDbError::Schema { detail, .. } if detail.contains("is_error")),
        "{err}"
    );
}

#[test]
fn a_database_without_the_tables_is_refused() {
    for (ddl, expected) in [
        (
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL NOT NULL); \
             INSERT INTO schema_migrations VALUES (1, 0.0);",
            "no events table",
        ),
        ("CREATE TABLE unrelated (x INTEGER);", "no schema_migrations table"),
    ] {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("trace.db");
        Connection::open(&path)
            .expect("create")
            .execute_batch(ddl)
            .expect("ddl");

        let err = trace_db_err(parse_trace_db(&trial(path)).unwrap_err());

        assert!(
            matches!(&err, TraceDbError::Schema { detail, .. } if detail == expected),
            "{expected}: {err}"
        );
    }
}

#[test]
fn a_file_that_is_not_a_database_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("trace.db");
    std::fs::write(&path, b"this is a text file, not SQLite\n".repeat(40)).expect("write");

    let err = trace_db_err(parse_trace_db(&trial(path)).unwrap_err());

    assert!(matches!(err, TraceDbError::Sqlite { .. }), "{err}");
    assert!(err.to_string().contains("not a database"), "{err}");
}

#[test]
fn an_absent_database_is_its_own_error() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("trace.db");

    let err = trace_db_err(parse_trace_db(&trial(path)).unwrap_err());

    assert!(matches!(err, TraceDbError::Absent { .. }), "{err}");
}

const ANSWER_ONLY: &str = "The fix is in src/lib.py.\n";
const WITH_EVENTS: &str = concat!(
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"from-transcript.py"}}]}}"#,
    "\n",
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"...","is_error":false}]}}"#,
    "\n",
);

fn transcript(dir: &TempDir, body: &str) -> PathBuf {
    let path = dir.path().join("agent_output.txt");
    std::fs::write(&path, body).expect("write transcript");
    path
}

fn read_paths(parsed: &aoa_codeprobe_shim::ParsedTrial) -> Vec<&str> {
    parsed
        .shim
        .trace
        .spans
        .iter()
        .filter(|s| s.span_type == SpanType::FileRead)
        .map(|s| target(s, "path"))
        .collect()
}

#[test]
fn parse_trial_prefers_a_transcript_that_carries_events() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());
    let transcript = transcript(&dir, WITH_EVENTS);

    let parsed = parse_trial(&transcript, Some(&trial)).expect("parses");

    assert_eq!(parsed.source, TraceSource::StreamJson);
    assert_eq!(read_paths(&parsed), vec!["from-transcript.py"]);
}

#[test]
fn parse_trial_falls_back_to_trace_db_for_an_answer_only_transcript() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());
    let transcript = transcript(&dir, ANSWER_ONLY);

    let parsed = parse_trial(&transcript, Some(&trial)).expect("parses");

    assert_eq!(parsed.source, TraceSource::TraceDb);
    assert_eq!(read_paths(&parsed), vec!["src/lib.py"]);
    assert_eq!(parsed.shim.warnings.len(), 1);
}

#[test]
fn parse_trial_keeps_no_agent_events_when_there_is_no_database() {
    let dir = TempDir::new().expect("tempdir");
    let transcript = transcript(&dir, ANSWER_ONLY);
    let absent = trial(dir.path().join("trace.db"));

    for trace_db in [None, Some(&absent)] {
        let err = parse_trial(&transcript, trace_db).unwrap_err();
        assert!(
            matches!(err, ShimError::NoAgentEvents { lines: 1 }),
            "{err}"
        );
    }
}

#[test]
fn parse_trial_reports_a_broken_database_instead_of_a_silent_agent() {
    let dir = TempDir::new().expect("tempdir");
    let transcript = transcript(&dir, ANSWER_ONLY);
    let empty = fixture(&dir, &[]);

    let err = trace_db_err(parse_trial(&transcript, Some(&empty)).unwrap_err());

    assert!(matches!(err, TraceDbError::NoEvents { .. }), "{err}");
}

#[test]
fn parse_trial_does_not_fall_back_for_a_missing_transcript() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());
    let missing = dir.path().join("agent_output.txt");

    let err = parse_trial(&missing, Some(&trial)).unwrap_err();

    assert!(
        matches!(&err, ShimError::Read { path, .. } if path == &missing),
        "{err}"
    );
}

#[test]
fn trace_db_file_name_is_what_the_layout_tests_build() {
    let dir = TempDir::new().expect("tempdir");
    let trial = fixture(&dir, &busy_rows());
    assert_eq!(
        trial.path.file_name(),
        Some(Path::new("trace.db").as_os_str())
    );
}
