/// Errors raised while reading a symbol index into a [`crate::IndexedRepo`].
///
/// These surface only from the source-specific entry points
/// ([`crate::index_with_scip`], [`crate::index_best_effort`]). The
/// degrade-on-failure path in [`crate::build_symbol_graph`] converts them into a
/// [`crate::IndexQuality::Degraded`] result instead of propagating.
#[derive(Debug, thiserror::Error)]
pub enum ScipGraphError {
    #[error("failed to read index source {path}: {source}", path = path.display())]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse SCIP index {path}: {source}", path = path.display())]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// An index source file exceeded its byte cap before being read into memory.
    /// Raised for attacker-controlled local files (a crafted SCIP index or a
    /// single oversized source file under a best-effort scan).
    #[error("index source {path} exceeds {max} byte cap (DoS guard)", path = path.display())]
    TooLarge { path: std::path::PathBuf, max: u64 },
}
