use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while installing telemetry (`observe`) or running the
/// read-only audit (`audit`).
#[derive(Debug, Error)]
pub enum AuditError {
    /// A filesystem operation against the repo failed.
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A trace produced through the observe-installed path failed validation.
    #[error(transparent)]
    Trace(#[from] aoa_trace::TraceError),

    /// Resolving or counting the context-file budget failed.
    #[error(transparent)]
    Budget(#[from] aoa_budget::BudgetError),

    /// Loading the observe-captured trace corpus (the behavioral-signal
    /// count) failed. A corrupt corpus file fails the audit loudly rather
    /// than under-counting the repo's held-out signal.
    #[error(transparent)]
    Corpus(#[from] aoa_observe_shim::ObserveShimError),
}
