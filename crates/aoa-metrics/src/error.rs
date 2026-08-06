use thiserror::Error;

/// Errors raised while computing AOA metrics from a trace and supporting inputs.
#[derive(Debug, Error)]
pub enum MetricError {
    /// Edit-locality requires at least two accepted solutions to form an
    /// intersection floor and a union ceiling.
    #[error("edit-locality needs >=2 accepted solutions, got {0}")]
    InsufficientAcceptedSolutions(usize),
    /// A retrieval span's `results` attribute was not an array of strings.
    /// Degrading it to an empty list would report broken instrumentation as a
    /// measured zero, and dropping a non-string entry would shift the rank of
    /// every entry after it — the rank MRR is read from.
    #[error("retrieval span `results` must be an array of strings, but {found}")]
    MalformedRankedResults { found: String },
}
