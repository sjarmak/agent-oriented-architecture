use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while installing telemetry (`observe`) or running the
/// read-only audit (`audit`).
///
/// Downstream callers must retain a fallback arm: new typed failures may be
/// added as the audit grows without turning every addition into a semver break.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuditError {
    /// A filesystem operation against the repo failed.
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A caller-supplied trace name could escape the installed `.aoa/traces`
    /// boundary. Rejected before any write: the name was absolute, empty, a
    /// `.`/`..` component, multi-component, or resolved through a symlink.
    #[error(
        "unsafe trace name {name:?}: must be a single filename with no path \
         separators, parent components, or symlink"
    )]
    UnsafeTraceName { name: String },

    /// A trace write targeted a path that already exists. Whole traces are
    /// finished artifacts, so the write is refused rather than truncating an
    /// observation the corpus may already have counted.
    #[error("trace file already exists at {path}: refusing to overwrite a landed trace")]
    TraceExists { path: PathBuf },

    /// A node of the installed `.aoa/traces` path already exists as a symlink.
    /// The install-path analogue of [`AuditError::UnsafeTraceName`]: that one
    /// guards the caller-supplied *name*, this one guards the *directories* the
    /// name is joined onto, which `create_dir_all` and `fs::write` would
    /// otherwise follow straight out of the repo.
    #[error(
        "refusing to install or write through an unsafe path at {path} \
         (AOA does not follow symlinks or accept non-relative components)"
    )]
    UnsafeInstallPath { path: PathBuf },

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
