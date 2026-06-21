use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while planning, applying, or rolling back a migration.
#[derive(Debug, Error)]
pub enum MigrateError {
    /// A filesystem operation against the checkout failed. Carries the path so a
    /// partial apply or rollback names the offending file (mirrors
    /// `aoa_audit::AuditError::Io` — there is no `From<io::Error>` on purpose).
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Surfacing the audit punch-list a fix consumes failed.
    #[error(transparent)]
    Audit(#[from] aoa_audit::AuditError),

    /// An `apply` targeted a path that unexpectedly already exists. Writes are
    /// create-exclusive: the migration never overwrites repo content silently,
    /// so an existing target is a hard error rather than a clobber.
    #[error("refusing to overwrite existing file at {path}: a create-only migration found it already present")]
    TargetExists { path: PathBuf },

    /// Serializing or deserializing the migration manifest failed.
    #[error("migration manifest at {path} is invalid: {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// A planned change targeted a path outside the repo being migrated. The
    /// engine refuses to write or archive outside the checkout, so a fix that
    /// returns an out-of-tree path is a hard error, never a silent escape.
    #[error("refusing to act on {path}: not inside the migrated repo {repo}")]
    PathOutsideRepo { path: PathBuf, repo: PathBuf },

    /// `apply` was called while a prior migration's manifest is still present.
    /// Overwriting it would destroy the only rollback record for that
    /// migration, so the engine refuses until it is rolled back.
    #[error("a migration is already applied (manifest at {manifest}); roll it back before applying again")]
    AlreadyApplied { manifest: PathBuf },

    /// Copying a file (archiving an original, or restoring it on rollback)
    /// failed. Carries both ends so the diagnostic names the real culprit.
    #[error("failed to copy {from} -> {to}: {source}")]
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A compiler-backed fix could not invoke its toolchain: `cargo`/`rustc` was
    /// not found on `PATH`, or the repo's pinned toolchain is not installed. A
    /// loud failure, never a silent empty plan — an absent toolchain must not be
    /// mistaken for "the repo already conforms" (that would corrupt the R0 gate).
    #[error("toolchain unavailable for the compiler-verified fix: {detail}")]
    ToolchainUnavailable { detail: String },

    /// The isolated build failed for an *infrastructure* reason — offline
    /// dependency resolution, an unreadable manifest — so no diagnostics could be
    /// produced. Distinct from [`RepoDoesNotCheck`](MigrateError::RepoDoesNotCheck):
    /// here the compiler never got far enough to certify anything. Loud, never an
    /// empty-plan swallow (H4).
    #[error("isolated build failed before diagnostics could be produced:\n{stderr}")]
    BuildFailed { stderr: String },

    /// The isolated build emitted error-level diagnostics unrelated to the fix's
    /// own lint class, so the tree does not cleanly compile and subtractivity
    /// cannot be certified. Refusing here (rather than applying a partial fix on a
    /// broken tree) keeps the repo-delta arm from silently diverging from a
    /// buildable baseline.
    #[error("repo does not cleanly check; cannot certify the compiler-verified fix:\n{stderr}")]
    RepoDoesNotCheck { stderr: String },

    /// Parsing the compiler's JSON diagnostics or applying the certified
    /// suggestions failed. This indicates malformed compiler output or a rustfix
    /// conflict — a real fault, surfaced rather than swallowed.
    #[error("failed to compute the compiler-verified fix: {message}")]
    FixCompute { message: String },
}
