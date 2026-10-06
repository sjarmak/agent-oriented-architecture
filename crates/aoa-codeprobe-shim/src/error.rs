use std::path::PathBuf;

use aoa_path_trust::PathTrustError;
use aoa_trace::SpanSource;

/// Errors produced while reading or parsing a codeprobe transcript.
///
/// Parsing is lenient at the line level (malformed lines are skipped and
/// surfaced as warnings on [`crate::ShimResult`], mirroring codeprobe's own
/// stream-json reader). The hard failures are being unable to read the file and
/// resource-bound breaches on attacker-controlled input — an oversized
/// transcript or a span count past the cap. Bound breaches fail loud rather than
/// silently truncating the trace, because the trace feeds R0 process metrics.
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

    #[error(transparent)]
    TraceDb(Box<TraceDbError>),
}

impl From<TraceDbError> for ShimError {
    fn from(err: TraceDbError) -> Self {
        Self::TraceDb(Box::new(err))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TraceDbError {
    #[error("no trace database at {path}")]
    Absent { path: PathBuf },

    #[error("failed to read trace database {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("refusing to read trace database {path}: {source}")]
    Refused {
        path: PathBuf,
        #[source]
        source: PathTrustError,
    },

    #[error("failed to copy trace database {path} into a private directory for reading: {source}")]
    Copy {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("trace database {path} and its write-ahead log exceed {max} byte cap (DoS guard)")]
    TooLarge { path: PathBuf, max: u64 },

    #[error("trace database {path}: {source}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("trace database {path}: {detail}")]
    Schema { path: PathBuf, detail: String },

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

    #[error(
        "trace database {path} holds no event for config {config:?} task {task_id:?}, \
         so no agent action is observable"
    )]
    NoEvents {
        path: PathBuf,
        config: String,
        task_id: String,
    },

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
