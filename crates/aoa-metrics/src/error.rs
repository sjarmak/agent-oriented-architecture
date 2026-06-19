use thiserror::Error;

/// Errors raised while computing AOA metrics from a trace and supporting inputs.
#[derive(Debug, Error)]
pub enum MetricError {
    /// Edit-locality requires at least two accepted solutions to form an
    /// intersection floor and a union ceiling.
    #[error("edit-locality needs >=2 accepted solutions, got {0}")]
    InsufficientAcceptedSolutions(usize),

    /// Mutation-surface depth `k` must be non-negative; the type enforces this,
    /// but a zero-node graph with a positive depth is reported explicitly.
    #[error("mutation-surface graph has no nodes")]
    EmptyGraph,
}
