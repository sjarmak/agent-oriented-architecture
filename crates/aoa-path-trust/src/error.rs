use std::path::PathBuf;

/// A name that is not one safe path component.
#[derive(Debug, thiserror::Error)]
#[error("{name:?} is not a single safe path component")]
pub struct UnsafePathComponent {
    pub name: String,
}

/// Why a caller-supplied path was refused under its trust root.
///
/// Callers map these onto their own error surface. The distinction that matters
/// is refusal (every variant but `Io`) versus an inconclusive filesystem
/// answer (`Io`): a trust check must never fold the latter into "safe".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathTrustError {
    #[error("refusing to follow a symlink at {path}")]
    Symlink { path: PathBuf },

    #[error("refusing {path}: a component of it is not a single relative name")]
    UnsafeComponent { path: PathBuf },

    #[error("refusing {path}: it exists but is not a regular file")]
    NotRegularFile { path: PathBuf },

    #[error("refusing {path}: resolving it passes through more than {limit} links")]
    TooManyLinks { path: PathBuf, limit: usize },

    /// The filesystem could not answer whether the node is safe. Never treated
    /// as absent: a check that fails open is not a check.
    #[error("filesystem operation failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl PathTrustError {
    /// The path the refusal or failure is about.
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Symlink { path }
            | Self::UnsafeComponent { path }
            | Self::NotRegularFile { path }
            | Self::TooManyLinks { path, .. }
            | Self::Io { path, .. } => path,
        }
    }

    pub(crate) fn symlink(path: impl Into<PathBuf>) -> Self {
        Self::Symlink { path: path.into() }
    }

    pub(crate) fn unsafe_component(path: impl Into<PathBuf>) -> Self {
        Self::UnsafeComponent { path: path.into() }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
