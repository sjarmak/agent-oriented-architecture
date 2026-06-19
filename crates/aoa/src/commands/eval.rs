use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use aoa_gap::RunResult;
use aoa_trace::TraceReport;

use crate::cli::{EvalArgs, EvalCommand};
use crate::commands::{eval_run, falsify_build, r0b};
use crate::output::{print_human, print_json};

/// Dispatch the eval sub-commands.
pub fn run(args: &EvalArgs) -> Result<i32> {
    match &args.command {
        EvalCommand::ValidateTrace(a) => validate_trace(&a.file, a.json),
        EvalCommand::Compare(a) => compare(&a.baseline, &a.migrated, a.json),
        EvalCommand::Run(a) => eval_run::run(a),
        EvalCommand::R0b(a) => r0b::run(a),
        EvalCommand::Experiment(a) => falsify_build::run(a),
    }
}

#[derive(Debug, Serialize)]
struct TraceView {
    total: usize,
    has_reconstructed: bool,
    counts: Vec<TypeCount>,
}

#[derive(Debug, Serialize)]
struct TypeCount {
    span_type: String,
    count: usize,
}

fn validate_trace(path: &Path, json: bool) -> Result<i32> {
    let report: TraceReport = aoa_trace::validate_trace(path)
        .with_context(|| format!("trace {} is invalid", path.display()))?;

    let counts: Vec<TypeCount> = report
        .counts()
        .iter()
        .map(|(span_type, count)| TypeCount {
            span_type: span_type.as_str().to_string(),
            count: *count,
        })
        .collect();

    let view = TraceView {
        total: report.total(),
        has_reconstructed: report.has_reconstructed(),
        counts,
    };

    if json {
        print_json(&view)?;
    } else {
        let mut out = String::new();
        let _ = writeln!(out, "trace valid: {} span(s)", view.total);
        for entry in &view.counts {
            let _ = writeln!(out, "  {:<16} {}", entry.span_type, entry.count);
        }
        let _ = writeln!(out, "  has_reconstructed: {}", view.has_reconstructed);
        print_human(&out);
    }
    Ok(0)
}

fn load_run(path: &Path) -> Result<RunResult> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read run file {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse run file {}", path.display()))
}

fn compare(baseline_path: &Path, migrated_path: &Path, json: bool) -> Result<i32> {
    let baseline = load_run(baseline_path)?;
    let migrated = load_run(migrated_path)?;

    let outcome =
        aoa_gap::compare(&baseline, &migrated).context("reward-hacking gap comparison failed")?;

    if json {
        print_json(&outcome)?;
    } else {
        print_human(&format!(
            "reward-hacking gap delta: {:+.4}\nheld-out delta: {:+.4}\nlabel: {:?}\n",
            outcome.gap_delta, outcome.held_out_delta, outcome.label,
        ));
    }
    Ok(0)
}
