use std::path::PathBuf;

use aoa_budget::BudgetError;
use thiserror::Error;

/// Errors raised while linting a context-file tree.
#[derive(Debug, Error)]
pub enum LintError {
    #[error(transparent)]
    Budget(#[from] BudgetError),

    #[error("failed to walk {dir} for context files")]
    Walk {
        dir: PathBuf,
        #[source]
        source: ignore::Error,
    },

    #[error("no context files to lint")]
    NoRoots,
}
