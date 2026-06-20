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
}
