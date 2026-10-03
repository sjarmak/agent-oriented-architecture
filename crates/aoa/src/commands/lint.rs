use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use aoa_budget::normalize_path;
use aoa_lint::ClosureBudget;
use serde::Serialize;

use crate::cli::LintArgs;
use crate::output::{print_human, print_json};

/// A finding projected to the wire form the CLI emits.
#[derive(Debug, Serialize)]
struct FindingView {
    file: PathBuf,
    category: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct LintView {
    roots: Vec<PathBuf>,
    budget: BudgetView,
    findings: Vec<FindingView>,
    suppressed: Vec<SuppressionView>,
}

#[derive(Debug, Serialize)]
struct BudgetView {
    tokenizer: String,
    reference_encoding: String,
    ceiling: usize,
    distinct_files: usize,
    target_tokens: usize,
    o200k_tokens: usize,
    closures: Vec<ClosureView>,
}

#[derive(Debug, Serialize)]
struct ClosureView {
    root: PathBuf,
    target_tokens: usize,
    o200k_tokens: usize,
    gating_target_tokens: usize,
    over_ceiling_by: usize,
    files: Vec<FileView>,
    outside_boundary: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct FileView {
    file: PathBuf,
    target_tokens: usize,
    o200k_tokens: usize,
    gating: bool,
}

impl ClosureView {
    fn new(closure: ClosureBudget, ceiling: usize) -> Self {
        Self {
            root: closure.root,
            target_tokens: closure.target_tokens,
            o200k_tokens: closure.o200k_tokens,
            gating_target_tokens: closure.gating_target_tokens,
            over_ceiling_by: closure.gating_target_tokens.saturating_sub(ceiling),
            files: closure
                .files
                .into_iter()
                .map(|file| FileView {
                    file: file.path,
                    target_tokens: file.target_tokens,
                    o200k_tokens: file.o200k_tokens,
                    gating: file.gating,
                })
                .collect(),
            outside_boundary: closure.outside_boundary,
        }
    }
}

#[derive(Debug, Serialize)]
struct SuppressionView {
    file: PathBuf,
    reason: String,
}

/// Lint context files and render findings. `--changed` restricts the reported
/// findings to that set; `# aoa-allow: oversized-context` suppressions surface
/// from the composed budget report.
pub fn run(args: &LintArgs) -> Result<i32> {
    let dir = linted_directory(args.root.as_deref());
    let roots = context_roots(args.root.as_deref(), dir)?;
    let report = aoa_lint::lint_context_roots(&roots, dir, &args.tokenizer)
        .context("failed to lint context")?;

    let changed: Option<BTreeSet<PathBuf>> = if args.changed.is_empty() {
        None
    } else {
        Some(args.changed.iter().cloned().collect())
    };

    let findings: Vec<FindingView> = report
        .findings
        .iter()
        .filter(|f| changed.as_ref().is_none_or(|set| set.contains(&f.file)))
        .map(|f| FindingView {
            file: f.file.clone(),
            category: f.category.id().to_string(),
            message: f.message.clone(),
        })
        .collect();

    let suppressed: Vec<SuppressionView> = report
        .budget
        .suppressions()
        .into_iter()
        .map(|(file, reason)| SuppressionView { file, reason })
        .collect();

    let budget = BudgetView {
        tokenizer: report.budget.target_model,
        reference_encoding: report.budget.reference_encoding,
        ceiling: args.ceiling,
        distinct_files: report.budget.files.len(),
        target_tokens: report.budget.target_tokens,
        o200k_tokens: report.budget.o200k_tokens,
        closures: report
            .closures
            .into_iter()
            .map(|closure| ClosureView::new(closure, args.ceiling))
            .collect(),
    };

    let view = LintView {
        roots,
        budget,
        findings,
        suppressed,
    };

    if args.json {
        print_json(&view)?;
    } else {
        print_human(&render_human(&view));
    }
    Ok(0)
}

fn linted_directory(root: Option<&Path>) -> &Path {
    root.and_then(Path::parent)
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn context_roots(root: Option<&Path>, dir: &Path) -> Result<Vec<PathBuf>> {
    let nested = aoa_lint::discover_context_roots(dir)
        .with_context(|| format!("failed to find context files under {}", dir.display()))?;

    let explicit = root.map(normalize_path);
    let roots: Vec<PathBuf> = explicit
        .iter()
        .cloned()
        .chain(
            nested
                .into_iter()
                .filter(|path| explicit.as_ref() != Some(path)),
        )
        .collect();
    if roots.is_empty() {
        bail!(
            "no AGENTS.md or CLAUDE.md found under {}; pass --root to name a context file",
            dir.display()
        );
    }
    Ok(roots)
}

fn render_human(view: &LintView) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "context lint: {} finding(s) across {} context root(s)",
        view.findings.len(),
        view.roots.len(),
    );
    let _ = writeln!(
        out,
        "  context budget ({}): {} tokens across {} file(s), ceiling {} per closure",
        view.budget.tokenizer,
        view.budget.target_tokens,
        view.budget.distinct_files,
        view.budget.ceiling,
    );
    for closure in &view.budget.closures {
        let breach = match closure.over_ceiling_by {
            0 => String::new(),
            over => format!(" [OVER CEILING by {over}]"),
        };
        let _ = writeln!(
            out,
            "    {}: {} tokens across {} file(s){}",
            closure.root.display(),
            closure.target_tokens,
            closure.files.len(),
            breach,
        );
        for file in &closure.files {
            let uncounted = if file.gating {
                ""
            } else {
                " (suppressed, not counted against the ceiling)"
            };
            let _ = writeln!(
                out,
                "      {}: {} tokens{}",
                file.file.display(),
                file.target_tokens,
                uncounted,
            );
        }
        for link in &closure.outside_boundary {
            let _ = writeln!(
                out,
                "      {}: not counted, it resolves outside the linted directory",
                link.display(),
            );
        }
    }
    for finding in &view.findings {
        let _ = writeln!(
            out,
            "  [{}] {} — {}",
            finding.category,
            finding.file.display(),
            finding.message,
        );
    }
    for suppression in &view.suppressed {
        let _ = writeln!(
            out,
            "  suppressed (oversized-context) {}: {}",
            suppression.file.display(),
            suppression.reason,
        );
    }
    out
}
