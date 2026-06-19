use aoa_budget::BudgetReport;
use serde::{Deserialize, Serialize};

use crate::finding::Finding;

/// A structured context-lint report.
///
/// Composes the aoa-budget closure result ([`budget`](LintReport::budget) — the
/// resolved file set and token budget) with the mechanical smell
/// [`findings`](LintReport::findings) in a single report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintReport {
    /// The composed aoa-budget report: resolved closure file set + token budget.
    pub budget: BudgetReport,
    /// Context-file smell findings over the closure's files.
    pub findings: Vec<Finding>,
}
