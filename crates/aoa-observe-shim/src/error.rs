use std::path::PathBuf;

use aoa_codeprobe_shim::ShimError;
use aoa_trace::TraceError;

/// Errors produced while loading the observe-captured trace corpus.
///
/// Every failure names the offending file. Corpus files are written by AOA's
/// own instrumentation (the enforce hooks, `write_trace`), so a file that does
/// not read, parse, or validate is upstream corruption and fails loud — never
/// a silent skip that would under-count the repo's behavioral signal.
#[derive(Debug, thiserror::Error)]
pub enum ObserveShimError {
    /// The traces directory (or an entry in it) could not be read.
    #[error("failed to scan trace corpus at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A corpus file could not be ingested (read cap, malformed span line).
    #[error("failed to ingest {path}: {source}")]
    Ingest {
        path: PathBuf,
        #[source]
        source: ShimError,
    },

    /// A `.json` trace file was not schema-valid trace JSON.
    #[error("trace file {path} is not schema-valid: {source}")]
    Schema {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// A parsed trace failed post-parse validation: an unsupported wire-format
    /// version ([`aoa_trace::TraceEnvelope::into_trace`]) or out-of-order spans
    /// ([`aoa_trace::validate_trace_value`]).
    #[error("trace {path} failed validation: {source}")]
    InvalidTrace {
        path: PathBuf,
        #[source]
        source: TraceError,
    },
}
