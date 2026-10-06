use std::path::PathBuf;

use aoa_trace::SpanSource;

/// Errors produced while reading or parsing a codeprobe trial.
///
/// Transcript parsing is lenient at the line level (malformed lines are skipped
/// and surfaced as warnings on [`crate::ShimResult`], mirroring codeprobe's own
/// stream-json reader). The hard failures are being unable to read the file and
/// resource-bound breaches on attacker-controlled input — an oversized
/// transcript or a span count past the cap. Bound breaches fail loud rather than
/// silently truncating the trace, because the trace feeds R0 process metrics.
///
/// Failures of the `trace.db` fallback source arrive boxed as
/// [`ShimError::TraceDb`]; [`TraceDbError`] enumerates them.
#[derive(Debug, thiserror::Error)]
pub enum ShimError {
    /// The transcript file could not be read from disk.
    #[error("failed to read transcript {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The transcript file exceeded the byte cap before parsing began.
    #[error("transcript {path} exceeds {max} byte cap (DoS guard)")]
    TranscriptTooLarge { path: PathBuf, max: u64 },

    /// The transcript would exceed the span cap. Failing here is deliberate: a
    /// silently truncated trace would feed wrong locality metrics.
    #[error("transcript exceeds the {max}-span cap (DoS guard)")]
    TooManySpans { max: usize },

    #[error(
        "transcript carries no stream-json `assistant` or `user` event \
         ({lines} non-blank line(s) read), so no agent action is observable"
    )]
    NoAgentEvents { lines: usize },

    /// A span-per-line log carried a line that is not a well-formed span.
    /// Raised by backends whose input format AOA itself owns (the observe
    /// live log): there a malformed line is upstream corruption and fails
    /// loud, unlike codeprobe transcript lines, which are lenient warnings
    /// because codeprobe interleaves non-JSON output.
    #[error("malformed span at line {line}: {source}")]
    MalformedSpan {
        /// 1-based line number of the offending line.
        line: usize,
        #[source]
        source: serde_json::Error,
    },

    /// A backend produced a trace that failed [`aoa_trace::validate_trace_value`].
    /// The conformance contract requires every backend's trace to validate, so
    /// this fails loud rather than admitting a malformed trace.
    #[error("backend produced an invalid trace: {0}")]
    InvalidTrace(#[from] aoa_trace::TraceError),

    /// A span's recorded provenance disagreed with the backend's declared
    /// posture. A backend that declares `native` must not emit `reconstructed`
    /// spans (or vice versa) — the conformance harness rejects the mismatch so
    /// provenance stays trustworthy for R7/R8 exclusion.
    #[error(
        "backend '{backend_id}' declares {declared:?} provenance but span {index} is {found:?}"
    )]
    ProvenanceMismatch {
        backend_id: &'static str,
        index: usize,
        declared: SpanSource,
        found: SpanSource,
    },

    /// A backend declared a contract version other than the live
    /// [`crate::CONTRACT_VERSION`]. The freshness gate rejects a backend that has
    /// drifted from the current contract revision.
    #[error(
        "backend '{backend_id}' targets contract {declared} but the live contract is {expected}"
    )]
    ContractVersionMismatch {
        backend_id: &'static str,
        declared: &'static str,
        expected: &'static str,
    },

    /// The run's `trace.db` could not be read as this trial's trace; the boxed
    /// [`TraceDbError`] says why. Boxed so the payload of the fallback path
    /// does not widen every `Result` in the crate and the crates that wrap it.
    #[error(transparent)]
    TraceDb(Box<TraceDbError>),
}

impl From<TraceDbError> for ShimError {
    fn from(err: TraceDbError) -> Self {
        Self::TraceDb(Box::new(err))
    }
}

/// Why a trial could not be read from the run's `trace.db`.
///
/// The reader ([`crate::parse_trace_db`]) is strict throughout: the database is
/// a schema AOA states once and accepts nothing else, so every departure from
/// it is its own loud failure rather than a warning.
#[derive(Debug, thiserror::Error)]
pub enum TraceDbError {
    /// No trace database exists at the path codeprobe would have written one to.
    /// [`crate::parse_trial`] treats this as "nothing to fall back to" and
    /// reports the transcript's own failure instead.
    #[error("no trace database at {path}")]
    Absent { path: PathBuf },

    /// The trace database could not be stat'ed for the size cap.
    #[error("failed to read trace database {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The trace database file exceeded the byte cap before it was opened.
    #[error("trace database {path} exceeds {max} byte cap (DoS guard)")]
    TooLarge { path: PathBuf, max: u64 },

    /// SQLite refused the database or a query against it: not a database file,
    /// a corrupt page, a column of the wrong type.
    #[error("trace database {path}: {source}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    /// The database opened but is not the schema this reader states it accepts:
    /// a missing table, another schema version, or other `events` columns.
    #[error("trace database {path}: {detail}")]
    Schema { path: PathBuf, detail: String },

    /// A row for the trial is not well-formed under the accepted schema.
    #[error(
        "trace database {path}: malformed event {event_seq} for config {config:?} \
         task {task_id:?}: {detail}"
    )]
    MalformedEvent {
        path: PathBuf,
        config: String,
        task_id: String,
        event_seq: i64,
        detail: String,
    },

    /// The database is well-formed but holds no event for the trial, so no
    /// agent action is observable from it either.
    #[error(
        "trace database {path} holds no event for config {config:?} task {task_id:?}, \
         so no agent action is observable"
    )]
    NoEvents {
        path: PathBuf,
        config: String,
        task_id: String,
    },

    /// More than one codeprobe run wrote events for the trial into the same
    /// database. The trial directory does not say which run produced it, so
    /// picking one would attribute another run's actions to this trial's score.
    #[error(
        "trace database {path} holds events for config {config:?} task {task_id:?} \
         from {} runs ({}): cannot tell which produced the trial",
        runs.len(),
        runs.join(", ")
    )]
    AmbiguousRun {
        path: PathBuf,
        config: String,
        task_id: String,
        runs: Vec<String>,
    },

    /// codeprobe's per-task trace budget overflowed under
    /// `--trace-overflow=truncate` and it dropped the trial's later events. The
    /// trace is incomplete, which the same reasoning as
    /// [`ShimError::TooManySpans`] refuses to feed into locality metrics.
    #[error(
        "trace database {path} marks config {config:?} task {task_id:?} truncated at \
         event {event_seq}: codeprobe dropped the later tool calls, so the trace is incomplete"
    )]
    Truncated {
        path: PathBuf,
        config: String,
        task_id: String,
        event_seq: i64,
    },
}
