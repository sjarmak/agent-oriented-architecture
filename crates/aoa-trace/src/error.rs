use std::path::PathBuf;

/// Errors produced while loading or validating a trace file.
#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    /// The trace file could not be read from disk.
    #[error("failed to read trace file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file was not structurally valid JSON matching the trace schema.
    #[error("trace file {path} is not schema-valid: {source}")]
    Schema {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Spans were not in monotonically non-decreasing `seq` order.
    #[error("trace spans are out of order at index {index}: seq {seq} < previous seq {previous}")]
    OutOfOrder {
        index: usize,
        seq: u64,
        previous: u64,
    },
}
