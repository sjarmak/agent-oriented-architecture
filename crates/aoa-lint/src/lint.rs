use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_budget::{
    count_budget, normalize_path, resolve_closure_within, resolve_contained_closure, BudgetReport,
    Closure, Config, FileBudget, UnreadLink,
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
    let boundary = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    lint_context_roots(&[root.to_path_buf()], &[], boundary, target_tokenizer)
}

pub fn lint_context_roots(
    named: &[PathBuf],
    discovered: &[PathBuf],
    boundary: &Path,
    target_tokenizer: &str,
) -> Result<LintReport, LintError> {
    let (merged, members) = resolve_members(named, discovered, boundary)?;
    let budget = count_budget(
        &merged,
        target_tokenizer,
        &Config::blocking(LINT_BUDGET_CEILING),
    )?;

    let findings = merged
        .files
        .iter()
        .flat_map(|file| {
            detectors::run_all(
                &LintedFile {
                    path: file.path.clone(),
                    text: file.text.clone(),
                },
                &merged.absent,
            )
        })
        .collect();

    let closures = members
        .into_iter()
        .map(|member| closure_budget(member, &budget))
        .collect();

    Ok(LintReport {
        budget,
        closures,
        findings,
    })
}

struct Member {
    root: PathBuf,
    paths: Vec<PathBuf>,
    outside_boundary: Vec<PathBuf>,
    unread: Vec<UnreadLink>,
}

fn resolve_members(
    named: &[PathBuf],
    discovered: &[PathBuf],
    boundary: &Path,
) -> Result<(Closure, Vec<Member>), LintError> {
    let first = named
        .first()
        .or(discovered.first())
        .ok_or(LintError::NoRoots)?;
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut absent = BTreeSet::new();
    let mut members = Vec::with_capacity(named.len() + discovered.len());
    let closures = named
        .iter()
        .map(|root| resolve_closure_within(root, boundary))
        .chain(
            discovered
                .iter()
                .map(|root| resolve_contained_closure(root, boundary)),
        );
    for closure in closures {
        let closure = closure?;
        members.push(Member {
            root: closure.root,
            paths: closure.files.iter().map(|file| file.path.clone()).collect(),
            outside_boundary: closure.outside_boundary,
            unread: closure.unread,
        });
        absent.extend(closure.absent);
        for file in closure.files {
            if seen.insert(file.path.clone()) {
                files.push(file);
            }
        }
    }
    let merged = Closure {
        root: normalize_path(first),
        files,
        outside_boundary: Vec::new(),
        unread: Vec::new(),
        absent,
    };
    Ok((merged, members))
}

fn closure_budget(member: Member, budget: &BudgetReport) -> ClosureBudget {
    let files: Vec<FileBudget> = member
        .paths
        .iter()
        .filter_map(|path| budget.files.iter().find(|file| &file.path == path))
        .cloned()
        .collect();
    let sum = |tokens: fn(&FileBudget) -> usize| files.iter().map(tokens).sum();
    ClosureBudget {
        root: member.root,
        o200k_tokens: sum(|file| file.o200k_tokens),
        target_tokens: sum(|file| file.target_tokens),
        gating_target_tokens: sum(|file| if file.gating { file.target_tokens } else { 0 }),
        files,
        outside_boundary: member.outside_boundary,
        unread: member.unread,
    }
}
