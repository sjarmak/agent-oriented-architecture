use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_budget::{
    count_budget, normalize_path, resolve_closure, BudgetReport, Closure, Config, FileBudget,
};

use crate::detectors::{self, LintedFile};
use crate::error::LintError;
use crate::report::{ClosureBudget, LintReport};

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
    lint_context_roots(&[root.to_path_buf()], target_tokenizer)
}

pub fn lint_context_roots(
    roots: &[PathBuf],
    target_tokenizer: &str,
) -> Result<LintReport, LintError> {
    let first = roots.first().ok_or(LintError::NoRoots)?;
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut members: Vec<(PathBuf, Vec<PathBuf>)> = Vec::with_capacity(roots.len());
    for root in roots {
        let closure = resolve_closure(root)?;
        members.push((
            closure.root,
            closure.files.iter().map(|file| file.path.clone()).collect(),
        ));
        for file in closure.files {
            if seen.insert(file.path.clone()) {
                files.push(file);
            }
        }
    }
    let merged = Closure {
        root: normalize_path(first),
        files,
    };
    let budget = count_budget(
        &merged,
        target_tokenizer,
        &Config::blocking(LINT_BUDGET_CEILING),
    )?;

    let findings = merged
        .files
        .iter()
        .flat_map(|file| {
            detectors::run_all(&LintedFile {
                path: file.path.clone(),
                text: file.text.clone(),
            })
        })
        .collect();

    let closures = members
        .into_iter()
        .map(|(root, paths)| closure_budget(root, &paths, &budget))
        .collect();

    Ok(LintReport {
        budget,
        closures,
        findings,
    })
}

fn closure_budget(root: PathBuf, paths: &[PathBuf], budget: &BudgetReport) -> ClosureBudget {
    let files: Vec<FileBudget> = paths
        .iter()
        .filter_map(|path| budget.files.iter().find(|file| &file.path == path))
        .cloned()
        .collect();
    let sum = |tokens: fn(&FileBudget) -> usize| files.iter().map(tokens).sum();
    ClosureBudget {
        root,
        o200k_tokens: sum(|file| file.o200k_tokens),
        target_tokens: sum(|file| file.target_tokens),
        gating_target_tokens: sum(|file| if file.gating { file.target_tokens } else { 0 }),
        files,
    }
}
