use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::error::{ShimError, TraceDbError};
use crate::parse::parse_transcript_file;
use crate::spans::{Limits, ShimResult, SpanBuilder};

pub const MAX_TRACE_DB_BYTES: u64 = 1024 * 1024 * 1024;

const SCHEMA_VERSION: i64 = 1;

const WAL_SUFFIX: &str = "-wal";

pub const TRACE_DB_SCHEMA: &str = "\
CREATE TABLE events (
    run_id        TEXT    NOT NULL,
    config        TEXT    NOT NULL,
    task_id       TEXT    NOT NULL,
    event_seq     INTEGER NOT NULL,
    ts            REAL    NOT NULL,
    event_type    TEXT    NOT NULL,
    tool_name     TEXT,
    tool_input    TEXT,
    tool_output   TEXT,
    duration_ms   INTEGER,
    input_tokens  INTEGER,
    output_tokens INTEGER,
    bytes_written INTEGER NOT NULL,
    PRIMARY KEY (run_id, config, task_id, event_seq)
);
CREATE TABLE schema_migrations (
    version    INTEGER PRIMARY KEY,
    applied_at REAL    NOT NULL
);
CREATE INDEX idx_events_config_task ON events (config, task_id);
CREATE INDEX idx_events_tool_name   ON events (tool_name);
CREATE INDEX idx_events_ts          ON events (ts);
INSERT INTO schema_migrations (version, applied_at) VALUES (1, 0.0);
";

struct Column {
    name: &'static str,
    declared_type: &'static str,
    not_null: bool,
    primary_key_ordinal: i64,
}

const fn column(
    name: &'static str,
    declared_type: &'static str,
    not_null: bool,
    primary_key_ordinal: i64,
) -> Column {
    Column {
        name,
        declared_type,
        not_null,
        primary_key_ordinal,
    }
}

const EVENTS_COLUMNS: [Column; 13] = [
    column("run_id", "TEXT", true, 1),
    column("config", "TEXT", true, 2),
    column("task_id", "TEXT", true, 3),
    column("event_seq", "INTEGER", true, 4),
    column("ts", "REAL", true, 0),
    column("event_type", "TEXT", true, 0),
    column("tool_name", "TEXT", false, 0),
    column("tool_input", "TEXT", false, 0),
    column("tool_output", "TEXT", false, 0),
    column("duration_ms", "INTEGER", false, 0),
    column("input_tokens", "INTEGER", false, 0),
    column("output_tokens", "INTEGER", false, 0),
    column("bytes_written", "INTEGER", true, 0),
];

const SCHEMA_MIGRATIONS_COLUMNS: [Column; 2] = [
    column("version", "INTEGER", false, 1),
    column("applied_at", "REAL", true, 0),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceDbTrial {
    pub path: PathBuf,
    pub config: String,
    pub task_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSource {
    StreamJson,
    TraceDb,
}

impl TraceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StreamJson => "stream-json",
            Self::TraceDb => "trace.db",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedTrial {
    pub shim: ShimResult,
    pub source: TraceSource,
}

pub fn parse_trial(
    transcript: &Path,
    trace_db: Option<&TraceDbTrial>,
) -> Result<ParsedTrial, ShimError> {
    let transcript_err = match parse_transcript_file(transcript) {
        Ok(shim) => {
            return Ok(ParsedTrial {
                shim,
                source: TraceSource::StreamJson,
            })
        }
        Err(err @ ShimError::NoAgentEvents { .. }) => err,
        Err(err) => return Err(err),
    };
    let Some(trial) = trace_db else {
        return Err(transcript_err);
    };
    match parse_trace_db(trial) {
        Ok(shim) => Ok(ParsedTrial {
            shim,
            source: TraceSource::TraceDb,
        }),
        Err(ShimError::TraceDb(err)) if matches!(*err, TraceDbError::Absent { .. }) => {
            Err(transcript_err)
        }
        Err(err) => Err(err),
    }
}

pub fn parse_trace_db(trial: &TraceDbTrial) -> Result<ShimResult, ShimError> {
    parse_trace_db_bounded(trial, MAX_TRACE_DB_BYTES, Limits::DEFAULT)
}

pub(crate) fn parse_trace_db_bounded(
    trial: &TraceDbTrial,
    max_bytes: u64,
    limits: Limits,
) -> Result<ShimResult, ShimError> {
    let copy = PrivateCopy::of(&trial.path, max_bytes)?;
    let conn = open_read_only(&copy.db_path(), &trial.path)?;
    check_schema(&conn, &trial.path)?;
    let events = load_events(&conn, trial, limits.max_spans)?;
    build_trace(trial, &events, limits)
}

struct PrivateCopy {
    dir: tempfile::TempDir,
}

impl PrivateCopy {
    fn of(source: &Path, max_bytes: u64) -> Result<Self, TraceDbError> {
        let wal = wal_path(source);
        let db_len = match std::fs::metadata(source) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(TraceDbError::Absent {
                    path: source.to_path_buf(),
                })
            }
            Err(err) => return Err(read_error(source, err)),
        };
        let wal_len = match std::fs::metadata(&wal) {
            Ok(meta) => Some(meta.len()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(read_error(&wal, err)),
        };
        if db_len.saturating_add(wal_len.unwrap_or(0)) > max_bytes {
            return Err(TraceDbError::TooLarge {
                path: source.to_path_buf(),
                max: max_bytes,
            });
        }
        let dir = tempfile::tempdir().map_err(|err| TraceDbError::Copy {
            path: source.to_path_buf(),
            source: err,
        })?;
        let copy = Self { dir };
        let copied = copy_capped(source, &copy.db_path(), max_bytes)?;
        if wal_len.is_some() {
            copy_capped(&wal, &wal_path(&copy.db_path()), max_bytes - copied)?;
        }
        Ok(copy)
    }

    fn db_path(&self) -> PathBuf {
        self.dir.path().join("trace.db")
    }
}

fn wal_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(WAL_SUFFIX);
    PathBuf::from(name)
}

fn read_error(path: &Path, source: std::io::Error) -> TraceDbError {
    TraceDbError::Read {
        path: path.to_path_buf(),
        source,
    }
}

fn copy_capped(source: &Path, destination: &Path, budget: u64) -> Result<u64, TraceDbError> {
    let mut reader = File::open(source)
        .map_err(|err| read_error(source, err))?
        .take(budget.saturating_add(1));
    let mut writer = File::create(destination).map_err(|err| TraceDbError::Copy {
        path: source.to_path_buf(),
        source: err,
    })?;
    let copied = std::io::copy(&mut reader, &mut writer).map_err(|err| TraceDbError::Copy {
        path: source.to_path_buf(),
        source: err,
    })?;
    if copied > budget {
        return Err(TraceDbError::TooLarge {
            path: source.to_path_buf(),
            max: budget,
        });
    }
    Ok(copied)
}

fn open_read_only(copy: &Path, source: &Path) -> Result<Connection, TraceDbError> {
    let db_err = |err| TraceDbError::Sqlite {
        path: source.to_path_buf(),
        source: err,
    };
    let conn = Connection::open_with_flags(
        copy,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_err)?;
    conn.pragma_update(None, "query_only", true)
        .map_err(db_err)?;
    Ok(conn)
}

fn check_schema(conn: &Connection, path: &Path) -> Result<(), TraceDbError> {
    check_table(conn, path, "schema_migrations", &SCHEMA_MIGRATIONS_COLUMNS)?;
    let versions: Vec<i64> = conn
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|source| TraceDbError::Sqlite {
            path: path.to_path_buf(),
            source,
        })?;
    if versions != [SCHEMA_VERSION] {
        return Err(TraceDbError::Schema {
            path: path.to_path_buf(),
            detail: format!(
                "schema_migrations holds versions {versions:?}; this reader accepts exactly [{SCHEMA_VERSION}]"
            ),
        });
    }
    check_table(conn, path, "events", &EVENTS_COLUMNS)
}

type ColumnShape = (String, String, bool, i64);

fn check_table(
    conn: &Connection,
    path: &Path,
    table: &str,
    expected: &[Column],
) -> Result<(), TraceDbError> {
    let found: Vec<ColumnShape> = conn
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)")
        .and_then(|mut stmt| {
            stmt.query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|source| TraceDbError::Sqlite {
            path: path.to_path_buf(),
            source,
        })?;
    if found.is_empty() {
        return Err(TraceDbError::Schema {
            path: path.to_path_buf(),
            detail: format!("no {table} table"),
        });
    }
    let expected: Vec<ColumnShape> = expected
        .iter()
        .map(|c| {
            (
                c.name.to_string(),
                c.declared_type.to_string(),
                c.not_null,
                c.primary_key_ordinal,
            )
        })
        .collect();
    if found != expected {
        return Err(TraceDbError::Schema {
            path: path.to_path_buf(),
            detail: format!(
                "{table} columns (name, type, not null, primary key ordinal) are {found:?}; \
                 this reader accepts exactly {expected:?}"
            ),
        });
    }
    Ok(())
}

struct EventRow {
    run_id: String,
    event_seq: i64,
    event_type: String,
    tool_name: Option<String>,
    tool_input: Option<String>,
}

fn load_events(
    conn: &Connection,
    trial: &TraceDbTrial,
    max_rows: usize,
) -> Result<Vec<EventRow>, ShimError> {
    let limit = i64::try_from(max_rows.saturating_add(1)).unwrap_or(i64::MAX);
    let rows = conn
        .prepare(
            "SELECT run_id, event_seq, event_type, tool_name, tool_input FROM events \
             WHERE config = ?1 AND task_id = ?2 ORDER BY event_seq LIMIT ?3",
        )
        .and_then(|mut stmt| {
            stmt.query_map(
                rusqlite::params![&trial.config, &trial.task_id, limit],
                |row| {
                    Ok(EventRow {
                        run_id: row.get(0)?,
                        event_seq: row.get(1)?,
                        event_type: row.get(2)?,
                        tool_name: row.get(3)?,
                        tool_input: row.get(4)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|source| TraceDbError::Sqlite {
            path: trial.path.clone(),
            source,
        })?;
    if rows.len() > max_rows {
        return Err(ShimError::TooManySpans { max: max_rows });
    }
    Ok(rows)
}

fn build_trace(
    trial: &TraceDbTrial,
    events: &[EventRow],
    limits: Limits,
) -> Result<ShimResult, ShimError> {
    if events.is_empty() {
        return Err(TraceDbError::NoEvents {
            path: trial.path.clone(),
            config: trial.config.clone(),
            task_id: trial.task_id.clone(),
        }
        .into());
    }
    let mut runs: Vec<&str> = events.iter().map(|e| e.run_id.as_str()).collect();
    runs.sort_unstable();
    runs.dedup();
    if runs.len() > 1 {
        return Err(TraceDbError::AmbiguousRun {
            path: trial.path.clone(),
            config: trial.config.clone(),
            task_id: trial.task_id.clone(),
            runs: runs.into_iter().map(str::to_owned).collect(),
        }
        .into());
    }
    let malformed = |event: &EventRow, detail: String| {
        ShimError::from(TraceDbError::MalformedEvent {
            path: trial.path.clone(),
            config: trial.config.clone(),
            task_id: trial.task_id.clone(),
            event_seq: event.event_seq,
            detail,
        })
    };
    let mut builder = SpanBuilder::new(limits);
    for event in events {
        match event.event_type.as_str() {
            "tool_use" => {
                let name = event
                    .tool_name
                    .as_deref()
                    .ok_or_else(|| malformed(event, "tool_use row has no tool_name".to_string()))?;
                let raw = event.tool_input.as_deref().ok_or_else(|| {
                    malformed(event, "tool_use row has no tool_input".to_string())
                })?;
                let input: Value = serde_json::from_str(raw)
                    .map_err(|e| malformed(event, format!("tool_input is not JSON: {e}")))?;
                if !input.is_object() {
                    return Err(malformed(
                        event,
                        "tool_input is not a JSON object".to_string(),
                    ));
                }
                builder.tool_use(None, name, &input)?;
            }
            "result" => {}
            "trace_truncated" => {
                return Err(TraceDbError::Truncated {
                    path: trial.path.clone(),
                    config: trial.config.clone(),
                    task_id: trial.task_id.clone(),
                    event_seq: event.event_seq,
                }
                .into());
            }
            other => {
                return Err(malformed(event, format!("unknown event_type {other:?}")));
            }
        }
    }
    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dir: &Path, rows: &[(&str, &str, &str)]) -> TraceDbTrial {
        let path = dir.join("trace.db");
        let conn = Connection::open(&path).expect("create fixture");
        conn.execute_batch(TRACE_DB_SCHEMA).expect("schema");
        for (seq, (event_type, tool_name, tool_input)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO events (run_id, config, task_id, event_seq, ts, event_type, \
                 tool_name, tool_input, bytes_written) VALUES ('r', 'c', 't', ?1, 0.0, ?2, ?3, ?4, 0)",
                rusqlite::params![seq as i64, event_type, tool_name, tool_input],
            )
            .expect("insert");
        }
        TraceDbTrial {
            path,
            config: "c".to_string(),
            task_id: "t".to_string(),
        }
    }

    fn too_large(err: &ShimError) -> Option<u64> {
        match err {
            ShimError::TraceDb(inner) => match **inner {
                TraceDbError::TooLarge { max, .. } => Some(max),
                _ => None,
            },
            _ => None,
        }
    }

    #[test]
    fn the_exported_ddl_is_the_schema_the_reader_accepts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let trial = fixture(dir.path(), &[("tool_use", "Read", r#"{"file_path":"a"}"#)]);
        parse_trace_db(&trial).expect("a database built from the exported DDL is accepted");
    }

    #[test]
    fn an_oversized_database_is_refused_before_it_is_opened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let trial = fixture(dir.path(), &[("tool_use", "Read", r#"{"file_path":"a"}"#)]);
        let err = parse_trace_db_bounded(&trial, 1, Limits::DEFAULT).unwrap_err();
        assert_eq!(too_large(&err), Some(1), "{err}");
    }

    #[test]
    fn the_cap_counts_the_write_ahead_log_with_the_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.db");
        let writer = Connection::open(&path).expect("create fixture");
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .expect("wal without checkpoints");
        writer.execute_batch(TRACE_DB_SCHEMA).expect("schema");
        for seq in 0..64 {
            writer
                .execute(
                    "INSERT INTO events (run_id, config, task_id, event_seq, ts, event_type, \
                     tool_name, tool_input, bytes_written) \
                     VALUES ('r', 'c', 't', ?1, 0.0, 'tool_use', 'Read', '{\"file_path\":\"a\"}', 0)",
                    [seq],
                )
                .expect("insert");
        }
        let db_len = std::fs::metadata(&path).expect("db").len();
        let wal_len = std::fs::metadata(wal_path(&path)).expect("wal").len();
        assert!(
            wal_len > 0,
            "the fixture must still hold its rows in the wal"
        );
        let trial = TraceDbTrial {
            path,
            config: "c".to_string(),
            task_id: "t".to_string(),
        };

        let err =
            parse_trace_db_bounded(&trial, db_len + wal_len - 1, Limits::DEFAULT).unwrap_err();
        assert_eq!(too_large(&err), Some(db_len + wal_len - 1), "{err}");

        let read = parse_trace_db_bounded(&trial, db_len + wal_len, Limits::DEFAULT)
            .expect("db plus wal within the cap is read");
        assert_eq!(read.trace.spans.len(), 65);
        drop(writer);
    }

    #[test]
    fn the_span_cap_fails_loud_instead_of_truncating() {
        let dir = tempfile::tempdir().expect("tempdir");
        let trial = fixture(
            dir.path(),
            &[
                ("tool_use", "Read", r#"{"file_path":"a"}"#),
                ("tool_use", "Read", r#"{"file_path":"b"}"#),
            ],
        );
        let limits = Limits {
            max_spans: 1,
            max_warnings: 1,
        };
        let err = parse_trace_db_bounded(&trial, MAX_TRACE_DB_BYTES, limits).unwrap_err();
        assert!(matches!(err, ShimError::TooManySpans { max: 1 }), "{err}");
    }

    #[test]
    fn rows_past_the_span_cap_are_never_loaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rows: Vec<(&str, &str, &str)> = (0..8)
            .map(|_| ("tool_use", "Read", r#"{"file_path":"a"}"#))
            .collect();
        let trial = fixture(dir.path(), &rows);
        let conn = open_read_only(&trial.path, &trial.path).expect("open");

        let capped = load_events(&conn, &trial, 3).map(|rows| rows.len());
        assert!(
            matches!(capped, Err(ShimError::TooManySpans { max: 3 })),
            "nine rows under a cap of three must fail"
        );
        let within = load_events(&conn, &trial, 8).map(|rows| rows.len());
        assert!(matches!(within, Ok(8)), "eight rows under a cap of eight");
    }
}
