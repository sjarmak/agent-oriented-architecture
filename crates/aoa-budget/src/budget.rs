use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::closure::Closure;
use crate::error::BudgetError;
use crate::suppress::find_suppression;
use crate::tokenizer::{count_tokens, reference_encoder, target_encoder, REFERENCE_ENCODING};

/// Configuration for a budget evaluation.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum allowed target-tokenizer token count for the gating set.
    pub ceiling: usize,
    /// When `true`, an over-budget closure yields [`Verdict::Warn`] instead of
    /// [`Verdict::Block`]. Default behavior (`false`) blocks.
    pub warn_first: bool,
    /// When `Some`, only files whose path is in this set count toward the gate
    /// (diff-scoped evaluation). Other files are still reported but do not gate.
    pub changed_files: Option<BTreeSet<PathBuf>>,
}

impl Config {
    /// A blocking config with the given ceiling and no diff scope.
    pub fn blocking(ceiling: usize) -> Self {
        Self {
            ceiling,
            warn_first: false,
            changed_files: None,
        }
    }

    /// A warn-first config with the given ceiling and no diff scope.
    pub fn warn_first(ceiling: usize) -> Self {
        Self {
            ceiling,
            warn_first: true,
            changed_files: None,
        }
    }
}

/// The outcome of a budget evaluation, gated on the target tokenizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Gating tokens are at or under the ceiling.
    Pass,
    /// Over the ceiling, but the config requested warn-first.
    Warn,
    /// Over the ceiling and blocking.
    Block,
}

/// Per-file token breakdown within a budget report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBudget {
    pub path: PathBuf,
    pub o200k_tokens: usize,
    pub target_tokens: usize,
    /// Whether this file participates in the gating sum (it is in scope and not
    /// suppressed).
    pub gating: bool,
    /// Captured reason if this file carried an oversized-context suppression.
    pub suppression: Option<String>,
}

/// A full budget report for a resolved closure under a target tokenizer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetReport {
    /// Total tokens across all closure files under the pinned reference
    /// encoding (o200k_base).
    pub o200k_tokens: usize,
    /// Total tokens across all closure files under the target tokenizer.
    pub target_tokens: usize,
    /// Target-token sum of only the gating files (in scope, not suppressed).
    /// This value is compared against the ceiling.
    pub gating_target_tokens: usize,
    /// The pinned reference encoding name.
    pub reference_encoding: String,
    /// The resolved target model / encoding name.
    pub target_model: String,
    /// Declared ceiling used for the gate.
    pub ceiling: usize,
    /// The gate outcome.
    pub verdict: Verdict,
    /// Per-file breakdown.
    pub files: Vec<FileBudget>,
}

impl BudgetReport {
    /// Reasons captured from oversized-context suppression markers, with their
    /// originating file path.
    pub fn suppressions(&self) -> Vec<(PathBuf, String)> {
        self.files
            .iter()
            .filter_map(|f| f.suppression.clone().map(|r| (f.path.clone(), r)))
            .collect()
    }
}

/// Count the budget of `closure` under the named `target` tokenizer, applying
/// `config` (ceiling, warn-first, diff scope, suppression) to reach a verdict.
///
/// The verdict is GATED on the target tokenizer: only the target-token sum of
/// in-scope, non-suppressed files is compared against the ceiling. Both the
/// reference (o200k_base) and target totals are always reported.
///
/// Returns [`BudgetError::UnknownTargetTokenizer`] when `target` is not a
/// supported tokenizer name — the gate fails loudly rather than guessing.
pub fn count_budget(
    closure: &Closure,
    target: &str,
    config: &Config,
) -> Result<BudgetReport, BudgetError> {
    let reference = reference_encoder()?;
    let target_enc = target_encoder(target)?;

    let mut files = Vec::with_capacity(closure.files.len());
    let mut o200k_total = 0usize;
    let mut target_total = 0usize;
    let mut gating_total = 0usize;

    for file in &closure.files {
        let o200k = count_tokens(&reference, &file.text);
        let target_tokens = count_tokens(&target_enc, &file.text);
        let suppression = find_suppression(&file.text);
        let in_scope = config
            .changed_files
            .as_ref()
            .map(|set| set.contains(&file.path))
            .unwrap_or(true);
        let gating = in_scope && suppression.is_none();

        o200k_total += o200k;
        target_total += target_tokens;
        if gating {
            gating_total += target_tokens;
        }

        files.push(FileBudget {
            path: file.path.clone(),
            o200k_tokens: o200k,
            target_tokens,
            gating,
            suppression,
        });
    }

    let verdict = if gating_total <= config.ceiling {
        Verdict::Pass
    } else if config.warn_first {
        Verdict::Warn
    } else {
        Verdict::Block
    };

    Ok(BudgetReport {
        o200k_tokens: o200k_total,
        target_tokens: target_total,
        gating_target_tokens: gating_total,
        reference_encoding: REFERENCE_ENCODING.to_string(),
        target_model: target.to_string(),
        ceiling: config.ceiling,
        verdict,
        files,
    })
}
