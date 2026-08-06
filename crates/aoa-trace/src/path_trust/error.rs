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
/// is refusal (`UnsafePath`, `EscapesRoot`) versus an inconclusive filesystem
/// answer (`Io`): a trust check must never fold the latter into "safe".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathTrustError {
    /// The path contains a component that cannot be joined safely, or a node of
    /// it already exists as a symlink the caller must not follow.
    #[error(
        "refusing to resolve through an unsafe path at {path} \
         (no symlinks, no non-relative components)"
    )]
    UnsafePath { path: PathBuf },

    /// Parent components walked above the trust root.
    #[error("{path} traverses above its trust root")]
    EscapesRoot { path: PathBuf },

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
            Self::UnsafePath { path } | Self::EscapesRoot { path } | Self::Io { path, .. } => path,
        }
    }

    pub(crate) fn unsafe_path(path: impl Into<PathBuf>) -> Self {
        Self::UnsafePath { path: path.into() }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
