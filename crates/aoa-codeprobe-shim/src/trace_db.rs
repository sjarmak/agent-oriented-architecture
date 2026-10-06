//! Read a trial's tool calls back out of codeprobe's `trace.db`.
//!
//! codeprobe's telemetry adapter ingests every trial's stream-json events into
//! one SQLite database per run (`<runs_dir>/trace.db`, beside the per-config
//! directories), and since it started doing so `agent_output.txt` holds only the
//! agent's extracted final answer. This module is the second trace source: when
//! the transcript carries no agent event, [`parse_trial`] reads the same
//! `tool_use` blocks from here and feeds them through the same
//! [`SpanBuilder`], so a tool call maps to the same span whichever file it was
//! read from.
//!
//! # Accepted schema
//!
//! Stated once, here, and accepted exactly. The writer is
//! `codeprobe/trace/store.py` (`SCHEMA_VERSION = 1`):
//!
//! ```sql
//! CREATE TABLE events (
//!     run_id        TEXT    NOT NULL,
//!     config        TEXT    NOT NULL,
//!     task_id       TEXT    NOT NULL,
//!     event_seq     INTEGER NOT NULL,
//!     ts            REAL    NOT NULL,
//!     event_type    TEXT    NOT NULL,
//!     tool_name     TEXT,
//!     tool_input    TEXT,
//!     tool_output   TEXT,
//!     duration_ms   INTEGER,
//!     input_tokens  INTEGER,
//!     output_tokens INTEGER,
//!     bytes_written INTEGER NOT NULL,
//!     PRIMARY KEY (run_id, config, task_id, event_seq)
//! );
//! CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL NOT NULL);
//! ```
//!
//! `schema_migrations` must hold exactly version 1 and `events` must have
//! exactly these columns, in this order, with these declared types and
//! nullability; anything else is [`TraceDbError::Schema`]. A trial is the
//! rows whose `config` is the run directory's name (codeprobe's
//! `experiment_config.label`) and whose `task_id` is the task directory's name,
//! in `event_seq` order. Three `event_type` values are known:
//!
//! | `event_type` | row | read as |
//! |--------------|-----|---------|
//! | `tool_use` | `tool_name`, `tool_input` = the block's `input` as JSON text | one tool call, via [`SpanBuilder::tool_use`] |
//! | `result` | token counts and the final answer | nothing: it carries no tool call |
//! | `trace_truncated` | codeprobe's budget marker; later events were dropped | [`TraceDbError::Truncated`] |
//!
//! Any other `event_type`, a `tool_use` row without a tool name, or a
//! `tool_input` that is not a JSON object is [`TraceDbError::MalformedEvent`].
//!
//! # What the database cannot say
//!
//! codeprobe records no `tool_result` rows, so a write's outcome is not
//! observable here: every write stays `write.attempt`. The transcript path
//! settles writes to `write.committed` / `write.blocked`; this one cannot, and
//! a consumer that needs the distinction must not get it from a trial read
//! this way.
//!
//! # Read-only
//!
//! The database is opened `SQLITE_OPEN_READ_ONLY` and additionally pinned with
//! `PRAGMA query_only`, so no statement this module could issue writes it. It is
//! not opened immutable: codeprobe writes in WAL mode and the frames it has not
//! yet checkpointed still belong to the trial, so the WAL is read as SQLite
//! intends.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::error::{ShimError, TraceDbError};
use crate::parse::parse_transcript_file;
use crate::spans::{Limits, ShimResult, SpanBuilder};

/// Largest trace database accepted. The whole run shares one file, so this is
/// a per-run bound, not a per-trial one; a canary run writes tens of KiB and a
/// hundred-trial run a few hundred MiB at codeprobe's default per-task budget.
pub const MAX_TRACE_DB_BYTES: u64 = 1024 * 1024 * 1024;

const SCHEMA_VERSION: i64 = 1;

/// The DDL of the schema this reader accepts, as codeprobe's `trace/store.py`
/// issues it (less the `IF NOT EXISTS` guards). Exported so a test fixture is
/// built from the statement the reader is checked against; the reader itself
/// never executes it.
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

const EVENTS_COLUMNS: [(&str, &str, bool); 13] = [
    ("run_id", "TEXT", true),
    ("config", "TEXT", true),
    ("task_id", "TEXT", true),
    ("event_seq", "INTEGER", true),
    ("ts", "REAL", true),
    ("event_type", "TEXT", true),
    ("tool_name", "TEXT", false),
    ("tool_input", "TEXT", false),
    ("tool_output", "TEXT", false),
    ("duration_ms", "INTEGER", false),
    ("input_tokens", "INTEGER", false),
    ("output_tokens", "INTEGER", false),
    ("bytes_written", "INTEGER", true),
];

/// Where one trial's events live: the database and the two key columns that
/// select them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceDbTrial {
    pub path: PathBuf,
    pub config: String,
    pub task_id: String,
}

/// Which file a trial's trace was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceSource {
    /// The `agent_output.txt` stream-json transcript.
    StreamJson,
    /// The run's `trace.db`.
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

/// A trial's trace together with the source it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedTrial {
    pub shim: ShimResult,
    pub source: TraceSource,
}

/// Parse a trial from its transcript, falling back to the run's `trace.db`.
///
/// The transcript wins when it carries agent events. When it parses to
/// [`ShimError::NoAgentEvents`] and `trace_db` names a database that exists,
/// the trial is read from there instead. Every other transcript failure, and an
/// absent database, reports the transcript's own error: a trial with neither
/// stream-json events nor a trace database is still `NoAgentEvents`. A database
/// that exists but cannot be read, or holds nothing for the trial, fails with
/// its own error so that a misread database is never mistaken for a silent
/// agent.
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

/// Read one trial's tool calls from `trace.db` into an ordered native trace.
///
/// Strict throughout: see the module documentation for the schema accepted and
/// the failures each departure from it raises.
pub fn parse_trace_db(trial: &TraceDbTrial) -> Result<ShimResult, ShimError> {
    parse_trace_db_bounded(trial, MAX_TRACE_DB_BYTES, Limits::DEFAULT)
}

pub(crate) fn parse_trace_db_bounded(
    trial: &TraceDbTrial,
    max_bytes: u64,
    limits: Limits,
) -> Result<ShimResult, ShimError> {
    check_size(&trial.path, max_bytes)?;
    let conn = open_read_only(&trial.path)?;
    check_schema(&conn, &trial.path)?;
    let events = load_events(&conn, trial)?;
    build_trace(trial, &events, limits)
}

fn check_size(path: &Path, max_bytes: u64) -> Result<(), TraceDbError> {
    let meta = std::fs::metadata(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            TraceDbError::Absent {
                path: path.to_path_buf(),
            }
        } else {
            TraceDbError::Read {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    if meta.len() > max_bytes {
        return Err(TraceDbError::TooLarge {
            path: path.to_path_buf(),
            max: max_bytes,
        });
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection, TraceDbError> {
    let db_err = |source| TraceDbError::Sqlite {
        path: path.to_path_buf(),
        source,
    };
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_err)?;
    conn.pragma_update(None, "query_only", true)
        .map_err(db_err)?;
    Ok(conn)
}

fn check_schema(conn: &Connection, path: &Path) -> Result<(), TraceDbError> {
    let db_err = |source| TraceDbError::Sqlite {
        path: path.to_path_buf(),
        source,
    };
    let schema_err = |detail: String| TraceDbError::Schema {
        path: path.to_path_buf(),
        detail,
    };
    let versions: Vec<i64> = conn
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|source| match source {
            rusqlite::Error::SqliteFailure(_, Some(ref msg)) if msg.contains("no such table") => {
                schema_err("no schema_migrations table".to_string())
            }
            other => db_err(other),
        })?;
    if versions != [SCHEMA_VERSION] {
        return Err(schema_err(format!(
            "schema_migrations holds versions {versions:?}; this reader accepts exactly [{SCHEMA_VERSION}]"
        )));
    }
    let columns: Vec<(String, String, bool)> = conn
        .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('events')")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(db_err)?;
    if columns.is_empty() {
        return Err(schema_err("no events table".to_string()));
    }
    let expected: Vec<(String, String, bool)> = EVENTS_COLUMNS
        .iter()
        .map(|(name, ty, notnull)| (name.to_string(), ty.to_string(), *notnull))
        .collect();
    if columns != expected {
        return Err(schema_err(format!(
            "events columns are {columns:?}; this reader accepts exactly {expected:?}"
        )));
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

fn load_events(conn: &Connection, trial: &TraceDbTrial) -> Result<Vec<EventRow>, TraceDbError> {
    conn.prepare(
        "SELECT run_id, event_seq, event_type, tool_name, tool_input FROM events \
         WHERE config = ?1 AND task_id = ?2 ORDER BY event_seq",
    )
    .and_then(|mut stmt| {
        stmt.query_map([&trial.config, &trial.task_id], |row| {
            Ok(EventRow {
                run_id: row.get(0)?,
                event_seq: row.get(1)?,
                event_type: row.get(2)?,
                tool_name: row.get(3)?,
                tool_input: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|source| TraceDbError::Sqlite {
        path: trial.path.clone(),
        source,
    })
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
        assert!(
            matches!(&err, ShimError::TraceDb(inner)
                if matches!(**inner, TraceDbError::TooLarge { max: 1, .. })),
            "{err}"
        );
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
}
