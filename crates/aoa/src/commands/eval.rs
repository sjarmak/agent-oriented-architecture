use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use aoa_gap::RunResult;
use aoa_trace::TraceReport;

use crate::cli::{EvalArgs, EvalCommand, ExposureCommand};
use crate::commands::fsutil::load_json_capped;
use crate::commands::{eval_run, exposure, falsify_build, r0b};
use crate::output::{print_human, print_json};

/// Dispatch the eval sub-commands.
pub fn run(args: &EvalArgs) -> Result<i32> {
    match &args.command {
        EvalCommand::ValidateTrace(a) => validate_trace(&a.file, a.json),
        EvalCommand::Compare(a) => compare(&a.baseline, &a.migrated, a.json),
        EvalCommand::Run(a) => eval_run::run(a),
        EvalCommand::R0b(a) => r0b::run(a),
        EvalCommand::Experiment(a) => falsify_build::run(a),
        EvalCommand::Exposure(a) => match &a.command {
            ExposureCommand::Scan(scan) => exposure::scan(scan),
        },
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
    // No path context here: every `TraceError` *returned by `validate_trace`*
    // names the offending file itself, so wrapping would print the path twice in
    // the `{err:#}` chain rendering. (The type carries no such guarantee — a bare
    // `UnsupportedVersion` from `into_trace` names nothing.)
    let report: TraceReport = aoa_trace::validate_trace(path)?;

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

/// Load a run-result JSON (byte-capped). Shared with the R14 self-audit, whose
/// held-out leg takes the same `--baseline`/`--migrated` inputs as `compare`.
pub(crate) fn load_run(path: &Path) -> Result<RunResult> {
    load_json_capped(path, "run file")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::fsutil::MAX_JSON_BYTES;

    #[test]
    fn load_run_rejects_oversized_input() {
        let dir = std::env::temp_dir().join(format!("aoa-eval-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.json");
        std::fs::write(&path, vec![b'x'; (MAX_JSON_BYTES + 1) as usize]).unwrap();

        let err = load_run(&path).unwrap_err();
        assert!(format!("{err:#}").contains("byte cap"), "got: {err:#}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
