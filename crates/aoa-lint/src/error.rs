use aoa_budget::BudgetError;
use thiserror::Error;

/// Errors raised while linting a context-file tree.
#[derive(Debug, Error)]
pub enum LintError {
    /// Resolving the context closure or counting its budget failed. The lint
    /// report composes the budget result, so a budget failure fails the lint.
    #[error("budget stage failed: {0}")]
    Budget(#[from] BudgetError),
}
