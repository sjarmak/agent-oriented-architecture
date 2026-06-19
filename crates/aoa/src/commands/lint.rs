use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
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
    let report = aoa_lint::lint_context(&args.root, &args.tokenizer)
        .with_context(|| format!("failed to lint context rooted at {}", args.root.display()))?;

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

fn render_human(view: &LintView) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "context lint: {} finding(s)", view.findings.len());
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
