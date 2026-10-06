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

    #[error("ignore file {path} is a link that leaves {dir}")]
    IgnoreFileOutside { path: PathBuf, dir: PathBuf },

    #[error("ignore file {path} is not a regular file")]
    IgnoreFileNotRegular { path: PathBuf },

    #[error("ignore file {path} has another name, so what it holds can be written from outside the linted directory")]
    IgnoreFileHardLinked { path: PathBuf },

    #[error("ignore file {path} is larger than {max_bytes} bytes")]
    IgnoreFileOversized { path: PathBuf, max_bytes: u64 },

    #[error("no context files to lint")]
    NoRoots,
}
