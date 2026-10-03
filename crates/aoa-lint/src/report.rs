use std::path::PathBuf;

use aoa_budget::{BudgetReport, FileBudget, UnreadLink};
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
    pub closures: Vec<ClosureBudget>,
    /// Context-file smell findings over the closure's files.
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureBudget {
    pub root: PathBuf,
    pub o200k_tokens: usize,
    pub target_tokens: usize,
    pub gating_target_tokens: usize,
    pub files: Vec<FileBudget>,
    pub outside_boundary: Vec<PathBuf>,
    pub unread: Vec<UnreadLink>,
}
