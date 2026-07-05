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

    /// The input's tasks carry convention inputs of more than one family. One
    /// experiment scores one task shape; a mixed input cannot be evaluated under
    /// a single admissible convention set.
    #[error("convention inputs mix task families; one experiment carries one task shape")]
    MixedInputFamilies,

    /// A configured convention's family does not match the family of the tasks'
    /// convention inputs. Scoring would silently admit nothing, so the mismatch
    /// is an input error, not a verdict.
    #[error(
        "convention '{name}' is {convention:?}-family but the task inputs are {input:?}-family"
    )]
    ConventionFamilyMismatch {
        name: String,
        convention: crate::convention::ConventionFamily,
        input: crate::convention::ConventionFamily,
    },

    /// The configured conventions are not, structurally, the pre-registered
    /// admissible set for the input's family. A hand-edited threshold, depth,
    /// weight, or a dropped/added convention would be invisible behind the
    /// convention names, so anything but the exact pre-registered set is an
    /// input error — there is no override.
    #[error(
        "configured conventions do not match the pre-registered {family:?}-family admissible \
         set; the pre-registered set is the only admissible set (no override)"
    )]
    ConventionSetNotPreRegistered {
        family: crate::convention::ConventionFamily,
    },
}
