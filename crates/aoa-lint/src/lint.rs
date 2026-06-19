use std::path::Path;

use aoa_budget::{count_budget, resolve_closure, Config};

use crate::detectors::{self, LintedFile};
use crate::error::LintError;
use crate::report::LintReport;

/// Informational ceiling for the composed budget report. Linting does not gate
/// on the token budget — it composes the budget result for visibility — so the
/// ceiling is set high enough not to drive a blocking verdict.
const LINT_BUDGET_CEILING: usize = usize::MAX;

/// Lint the context-file tree rooted at `root` for config-file smells, composing
/// the aoa-budget closure result (resolved file set + token budget under
/// `target_tokenizer`) with the smell findings into a single [`LintReport`].
///
/// The closure resolved by aoa-budget defines WHICH files are linted: every file
/// reachable from `root` is run through the mechanical detectors.
pub fn lint_context(root: &Path, target_tokenizer: &str) -> Result<LintReport, LintError> {
    let closure = resolve_closure(root)?;
    let budget = count_budget(
        &closure,
        target_tokenizer,
        &Config::blocking(LINT_BUDGET_CEILING),
    )?;

    let findings = closure
        .files
        .iter()
        .flat_map(|file| {
            detectors::run_all(&LintedFile {
                path: file.path.clone(),
                text: file.text.clone(),
            })
        })
        .collect();

    Ok(LintReport { budget, findings })
}
