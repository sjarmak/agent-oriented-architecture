use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use aoa_path_trust::{open_regular_file_nofollow, PathTrustError};
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
        let Some(db) = open_source(source)? else {
            return Err(TraceDbError::Absent {
                path: source.to_path_buf(),
            });
        };
        let wal = wal_path(source);
        let wal_file = open_source(&wal)?;
        let db_len = length_of(&db, source)?;
        let wal_len = match &wal_file {
            Some(file) => length_of(file, &wal)?,
            None => 0,
        };
        if db_len.saturating_add(wal_len) > max_bytes {
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
        let copied = copy_capped(db, source, &copy.db_path(), max_bytes, max_bytes)?;
        if let Some(wal_file) = wal_file {
            copy_capped(
                wal_file,
                &wal,
                &wal_path(&copy.db_path()),
                max_bytes - copied,
                max_bytes,
            )?;
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

fn open_source(path: &Path) -> Result<Option<File>, TraceDbError> {
    open_regular_file_nofollow(path).map_err(|err| match err {
        PathTrustError::Io { source, .. } => read_error(path, source),
        refused => TraceDbError::Refused {
            path: path.to_path_buf(),
            source: refused,
        },
    })
}

fn length_of(file: &File, path: &Path) -> Result<u64, TraceDbError> {
    file.metadata()
        .map(|meta| meta.len())
        .map_err(|err| read_error(path, err))
}

fn read_error(path: &Path, source: std::io::Error) -> TraceDbError {
    TraceDbError::Read {
        path: path.to_path_buf(),
        source,
    }
}

fn copy_capped(
    source_file: File,
    source: &Path,
    destination: &Path,
    budget: u64,
    cap: u64,
) -> Result<u64, TraceDbError> {
    let mut reader = source_file.take(budget.saturating_add(1));
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
            max: cap,
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
    let sqlite_err = |source| TraceDbError::Sqlite {
        path: path.to_path_buf(),
        source,
    };
    let found = schema_objects(conn).map_err(sqlite_err)?;
    let expected = Connection::open_in_memory()
        .and_then(|reference| {
            reference.execute_batch(TRACE_DB_SCHEMA)?;
            schema_objects(&reference)
        })
        .map_err(sqlite_err)?;
    if found != expected {
        let missing: Vec<_> = expected
            .iter()
            .filter(|o| !found.contains(o))
            .map(shown)
            .collect();
        let unexpected: Vec<_> = found
            .iter()
            .filter(|o| !expected.contains(o))
            .map(shown)
            .collect();
        let detail = format!(
            "sqlite_master rows (type, name, table, sql) differ from the accepted schema \
             (at most {SCHEMA_OBJECTS_FETCHED} objects read, each field shown to \
             {FIELD_SHOWN} bytes, message to {DETAIL_SHOWN} bytes): \
             missing {missing:?}; unexpected {unexpected:?}"
        );
        return Err(TraceDbError::Schema {
            path: path.to_path_buf(),
            detail: match cut(&detail, DETAIL_SHOWN) {
                Some(kept) => format!(
                    "{kept}... (message cut at {DETAIL_SHOWN} of {} bytes)",
                    detail.len()
                ),
                None => detail,
            },
        });
    }
    let versions: Vec<i64> = conn
        .prepare("SELECT version FROM schema_migrations ORDER BY version LIMIT 2")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sqlite_err)?;
    if versions != [SCHEMA_VERSION] {
        return Err(TraceDbError::Schema {
            path: path.to_path_buf(),
            detail: format!(
                "schema_migrations holds versions {versions:?} (at most two listed); \
                 this reader accepts exactly [{SCHEMA_VERSION}]"
            ),
        });
    }
    Ok(())
}

type SchemaObject = (String, String, String, Option<String>);

const SCHEMA_OBJECTS_FETCHED: i64 = 16;

const FIELD_SHOWN: usize = 512;

const DETAIL_SHOWN: usize = 8 * 1024;

fn shown(object: &SchemaObject) -> (String, String, String, Option<String>) {
    let (kind, name, table, sql) = object;
    let field = |text: &str| match cut(text, FIELD_SHOWN) {
        Some(kept) => format!("{kept}... ({} bytes)", text.len()),
        None => text.to_string(),
    };
    (
        field(kind),
        field(name),
        field(table),
        sql.as_deref().map(field),
    )
}

fn cut(text: &str, limit: usize) -> Option<&str> {
    (text.len() > limit).then(|| &text[..text.floor_char_boundary(limit)])
}

fn schema_objects(conn: &Connection) -> Result<Vec<SchemaObject>, rusqlite::Error> {
    conn.prepare(
        "SELECT type, name, tbl_name, sql FROM sqlite_master \
         ORDER BY type, name LIMIT ?1",
    )?
    .query_map([SCHEMA_OBJECTS_FETCHED], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get::<_, Option<String>>(3)?
                .map(|sql| normalize_sql(&sql)),
        ))
    })?
    .collect()
}

fn is_sqlite_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r')
}

fn normalize_sql(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for token in sql.split(is_sqlite_whitespace).filter(|t| !t.is_empty()) {
        if !out.is_empty() && !out.ends_with(['(', ',']) && !token.starts_with(['(', ')', ',']) {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
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
    fn sql_text_differing_only_in_whitespace_normalizes_to_one_spelling() {
        let multi_line = normalize_sql(
            "CREATE TABLE schema_migrations (\n    version    INTEGER PRIMARY KEY,\n    applied_at REAL    NOT NULL\n)",
        );
        let one_line = normalize_sql(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL NOT NULL)",
        );
        let spaced_out = normalize_sql(
            "CREATE  TABLE schema_migrations ( version INTEGER PRIMARY KEY ,applied_at REAL NOT NULL )",
        );
        assert_eq!(multi_line, one_line);
        assert_eq!(multi_line, spaced_out);
        assert_eq!(normalize_sql(&multi_line), multi_line);
        assert_ne!(
            one_line,
            normalize_sql(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL)"
            )
        );
        assert_ne!(
            normalize_sql("config TEXT NOT NULL"),
            normalize_sql("config TEXT NOT NULL COLLATE NOCASE")
        );
    }

    #[test]
    fn only_the_whitespace_sqlite_tokenizes_as_whitespace_is_collapsed() {
        assert_eq!(
            normalize_sql(" \t\n\x0c\rconfig \t\n\x0c\rTEXT \t\n\x0c\rNOT NULL \t\n\x0c\r"),
            "config TEXT NOT NULL"
        );
        assert_eq!(
            normalize_sql("config TEXT\u{a0}NOT  NULL"),
            "config TEXT\u{a0}NOT NULL"
        );
        assert_ne!(
            normalize_sql("config TEXT\u{a0}NOT NULL"),
            normalize_sql("config TEXT NOT NULL")
        );
        for other in ['\u{a0}', '\u{85}', '\u{2003}', '\u{3000}', '\x0b'] {
            let spelled = format!("config{other}TEXT");
            assert_eq!(normalize_sql(&spelled), spelled, "{other:?}");
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
    fn a_write_ahead_log_that_outgrows_its_budget_names_the_whole_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wal = dir.path().join("trace.db-wal");
        std::fs::write(&wal, [0u8; 16]).expect("wal");

        let file = File::open(&wal).expect("open wal");
        let err = copy_capped(file, &wal, &dir.path().join("copy"), 8, 32).unwrap_err();

        assert!(
            matches!(err, TraceDbError::TooLarge { max: 32, .. }),
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
