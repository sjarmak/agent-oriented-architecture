use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use aoa_budget::normalize_path;
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

/// The CLI lint result: smell findings (optionally restricted to changed files)
/// plus the suppression reasons captured from the composed budget report.
#[derive(Debug, Serialize)]
struct LintView {
    roots: Vec<PathBuf>,
    findings: Vec<FindingView>,
    suppressed: Vec<SuppressionView>,
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
    let roots = context_roots(args.root.as_deref())?;
    let report =
        aoa_lint::lint_context_roots(&roots, &args.tokenizer).context("failed to lint context")?;

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

    let view = LintView {
        roots,
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

fn context_roots(root: Option<&Path>) -> Result<Vec<PathBuf>> {
    let dir = root
        .and_then(Path::parent)
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
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
