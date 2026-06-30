use std::path::PathBuf;

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
}
