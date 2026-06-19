/// Errors raised while reading a symbol index into a [`crate::IndexedRepo`].
///
/// These surface only from the source-specific entry points
/// ([`crate::index_with_scip`], [`crate::index_best_effort`]). The
/// degrade-on-failure path in [`crate::build_symbol_graph`] converts them into a
/// [`crate::IndexQuality::Degraded`] result instead of propagating.
#[derive(Debug, thiserror::Error)]
pub enum ScipGraphError {
    #[error("failed to read index source {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse SCIP index {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}
