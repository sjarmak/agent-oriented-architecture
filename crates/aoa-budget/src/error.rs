use std::path::PathBuf;

use thiserror::Error;

/// Errors raised while resolving, counting, or fixing a context budget.
#[derive(Debug, Error)]
pub enum BudgetError {
    /// A context file could not be read from disk.
    #[error("failed to read context file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The requested target-model tokenizer name is not supported.
    ///
    /// This is raised loudly (never silently defaulted) so a misconfigured
    /// target model fails the gate instead of being scored against a guessed
    /// encoding.
    #[error("unknown target tokenizer '{name}' (supported: {supported})")]
    UnknownTargetTokenizer { name: String, supported: String },

    /// A `fix` operation ran but the resulting closure is still over budget.
    #[error("fix did not bring closure under ceiling: {target_tokens} target tokens >= ceiling {ceiling}")]
    FixFailed {
        target_tokens: usize,
        ceiling: usize,
    },
}
