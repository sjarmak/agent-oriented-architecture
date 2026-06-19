use thiserror::Error;

/// Structural failures that prevent the falsification gate from running at all.
///
/// These are distinct from verdict downgrades: a downgrade (`proceed` to
/// `inconclusive`) is a legitimate, data-carrying outcome, not an error. An
/// error here means the input cannot be evaluated under the R0 contract.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FalsifyError {
    /// R0 requires at least five repos to reason about a majority. Fewer than
    /// five cannot establish the cross-repo evidence the gate is built on.
    #[error("R0 requires at least 5 repos, got {0}")]
    TooFewRepos(usize),

    /// A repo carried no run snapshots, so its verdict cannot be checked for
    /// determinism across the configured `k_runs`.
    #[error("repo {repo_id} has no run snapshots")]
    EmptyRuns { repo_id: String },

    /// The configured determinism replication count must be at least three.
    #[error("k_runs must be >= 3, got {0}")]
    InsufficientReplication(u32),
}
