//! `aoa eval r0b`: compose the AOA held-out-integrity leakage canary over two
//! live codeprobe runs (baseline vs migrated).
//!
//! Where `eval run` (aoa-2lw) emits per-task process metrics from a single run,
//! this gate operates at the RUN level across many tasks and is the live-data
//! composition of `aoa_gap`'s already-tested R0b checks: it aggregates each run
//! into an `aoa_gap::RunResult` (held-out provenance + per-task outcomes +
//! injected canaries) and hands the pair to `aoa_gap::compare`, which rejects a
//! synthesized held-out suite, refuses to label on an absent gap, and trips when
//! the held-out rate rises without visible movement while a known canary flips.
//!
//! # Visible vs held-out on codeprobe data
//!
//! A codeprobe trial has no spec the agent literally reads back as a test suite,
//! so the two independent signals come from codeprobe's **dual-verifier** scorer
//! (`scorer_family == "dual_composite"`):
//! - **held-out** = the ARTIFACT leg (`passed_artifact`): the agent's `answer.json`
//!   compared against the mined `ground_truth.json`. This is the contamination-free
//!   oracle — mined from repo history, never derived from the visible spec.
//! - **visible** = the DIRECT leg (`passed_direct`): `test.sh` run against the
//!   agent's diff. It is the gameable *proxy* verifier (a migration can make the
//!   runnable test pass without reproducing the full mined change), NOT a suite
//!   the agent sees. The reward-hacking gap `visible - held_out` is exactly the
//!   "passes the runnable proxy but not the true oracle" signature.
//!
//! A run whose `scoring.json` is not `dual_composite` (or whose legs errored) has
//! no independent visible leg, so R0b **fails loud** rather than fabricating one.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use aoa_bench::load_task;
use aoa_gap::{
    compare, CanaryItem, CompareOutcome, CompareWarning, GapError, HeldOutProvenance, Label,
    RunResult, TaskOutcome,
};

use crate::cli::R0bArgs;
use crate::commands::codeprobe::{aggregate_provenance, discover_tasks, DualScoring};
use crate::output::{print_human, print_json};

/// One operator-declared canary: a known held-out probe and the outcome a clean
/// (non-leaking) run must produce for it.
#[derive(Debug, Deserialize)]
struct CanarySpec {
    id: String,
    expected_held_out: bool,
}

/// Parse the canary manifest into an id → expected-held-out map.
fn load_canary_manifest(path: &Path) -> Result<BTreeMap<String, bool>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read canary manifest {}", path.display()))?;
    let specs: Vec<CanarySpec> = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse canary manifest {}", path.display()))?;

    let mut map = BTreeMap::new();
    for spec in specs {
        if map.contains_key(&spec.id) {
            bail!("duplicate canary id {} in manifest", spec.id.escape_debug());
        }
        map.insert(spec.id, spec.expected_held_out);
    }
    Ok(map)
}

/// Aggregate one codeprobe run directory into a run-level `aoa_gap::RunResult`.
fn aggregate_run(
    run_dir: &Path,
    tasks_dir: &Path,
    canary_map: &BTreeMap<String, bool>,
) -> Result<RunResult> {
    let task_ids = discover_tasks(run_dir)?;

    // Validate canary membership up front, before any per-task I/O: every
    // declared canary must name a real trial in this run, or the leakage check
    // would silently lose its known-item signal. The id is untrusted manifest
    // text, so escape it in the error.
    for id in canary_map.keys() {
        if !task_ids.iter().any(|t| t == id) {
            bail!(
                "canary id {} is not a trial in run {} (declared in the manifest but absent)",
                id.escape_debug(),
                run_dir.display()
            );
        }
    }

    let mut tasks = Vec::with_capacity(task_ids.len());
    let mut provenances = Vec::with_capacity(task_ids.len());
    let mut canaries = Vec::new();

    for task_id in &task_ids {
        let task_dir = run_dir.join(task_id);
        let scoring_path = task_dir.join("scoring.json");
        let scoring = DualScoring::load(&scoring_path, task_id)?;

        let visible_success = scoring.visible_success(task_id)?;
        let held_out_success = scoring.held_out_success(task_id)?;

        let task = load_task(tasks_dir.join(task_id)).with_context(|| {
            format!(
                "failed to load task {} oracle from {}",
                task_id.escape_debug(),
                tasks_dir.display()
            )
        })?;
        provenances.push(task.held_out_provenance());

        tasks.push(TaskOutcome {
            visible_success,
            held_out_success,
        });

        // A declared canary's observed held-out outcome is read straight from the
        // run; only `expected_held_out` is operator-supplied. A divergence is the
        // flip `aoa_gap` keys leakage detection on.
        if let Some(&expected_held_out) = canary_map.get(task_id) {
            canaries.push(CanaryItem {
                id: task_id.clone(),
                held_out_success,
                expected_held_out,
            });
        }
    }

    let held_out_provenance = aggregate_provenance(&provenances)?;
    Ok(RunResult {
        tasks,
        held_out_provenance,
        canaries,
    })
}

/// Run-level figures surfaced for both registers.
#[derive(Debug, Serialize)]
struct RunView {
    run_dir: String,
    task_count: usize,
    visible_rate: f64,
    held_out_rate: f64,
    held_out_provenance: HeldOutProvenance,
    canary_count: usize,
    canary_flipped: bool,
}

impl RunView {
    fn of(run: &RunResult, run_dir: &Path) -> Self {
        RunView {
            run_dir: run_dir.display().to_string(),
            task_count: run.tasks.len(),
            visible_rate: run.visible_rate(),
            held_out_rate: run.held_out_rate(),
            held_out_provenance: run.held_out_provenance,
            canary_count: run.canaries.len(),
            canary_flipped: run.any_canary_flipped(),
        }
    }
}

/// The verdict half of the report: either a computed label or a loud refusal.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Verdict {
    Labeled {
        gap_delta: f64,
        held_out_delta: f64,
        label: Label,
        /// Non-fatal advisories from the comparison; notably a zero-canary
        /// leak-shaped signal the gate could not adjudicate. Empty in the clean
        /// case and omitted from JSON when empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<CompareWarning>,
    },
    Refused {
        kind: String,
        error: String,
    },
}

#[derive(Debug, Serialize)]
struct R0bReport {
    baseline: RunView,
    migrated: RunView,
    #[serde(flatten)]
    verdict: Verdict,
}

/// Map a comparison warning to a stable machine kind for the human report.
fn warning_kind(warning: &CompareWarning) -> &'static str {
    match warning {
        CompareWarning::ZeroCanaryLeakShape => "zero_canary_leak_shape",
    }
}

/// Operator-facing explanation of a comparison warning.
fn warning_detail(warning: &CompareWarning) -> &'static str {
    match warning {
        CompareWarning::ZeroCanaryLeakShape => {
            "held-out rose while visible stayed flat (the leak signature) but no \
             canary was injected, so leakage could not be confirmed — inject \
             canaries for this suite"
        }
    }
}

/// Map a gap-comparison error to a stable machine kind for the report.
fn refusal_kind(err: &GapError) -> &'static str {
    match err {
        GapError::SynthesizedHeldOut => "synthesized_held_out",
        GapError::LeakageDetected => "leakage_detected",
        GapError::GapUnavailable => "gap_unavailable",
    }
}

/// Post-process a baseline/migrated codeprobe run pair through the R0b gate.
pub fn run(args: &R0bArgs) -> Result<i32> {
    let canary_map = match &args.canary {
        Some(path) => load_canary_manifest(path)?,
        None => BTreeMap::new(),
    };

    let baseline = aggregate_run(&args.baseline, &args.tasks, &canary_map)
        .context("failed to aggregate baseline run")?;
    let migrated = aggregate_run(&args.migrated, &args.tasks, &canary_map)
        .context("failed to aggregate migrated run")?;

    let (verdict, code) = match compare(&baseline, &migrated) {
        Ok(CompareOutcome {
            gap_delta,
            held_out_delta,
            label,
            warnings,
        }) => (
            Verdict::Labeled {
                gap_delta,
                held_out_delta,
                label,
                warnings,
            },
            0,
        ),
        // A refusal (leakage / unavailable / synthesized) is a gate FAILURE: exit
        // non-zero so a CI gate or downstream R0 experiment blocks, with the root
        // cause named rather than collapsed.
        Err(err) => (
            Verdict::Refused {
                kind: refusal_kind(&err).to_string(),
                error: err.to_string(),
            },
            1,
        ),
    };

    let report = R0bReport {
        baseline: RunView::of(&baseline, &args.baseline),
        migrated: RunView::of(&migrated, &args.migrated),
        verdict,
    };

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&render_human(&report));
    }
    Ok(code)
}

fn render_run(out: &mut String, label: &str, view: &RunView) {
    let _ = writeln!(
        out,
        "  {label:<8} visible={:.3} held_out={:.3} provenance={:?} canaries={} flipped={} ({})",
        view.visible_rate,
        view.held_out_rate,
        view.held_out_provenance,
        view.canary_count,
        view.canary_flipped,
        view.run_dir,
    );
}

fn render_human(report: &R0bReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "aoa eval r0b:");
    render_run(&mut out, "baseline", &report.baseline);
    render_run(&mut out, "migrated", &report.migrated);
    match &report.verdict {
        Verdict::Labeled {
            gap_delta,
            held_out_delta,
            label,
            warnings,
        } => {
            let _ = writeln!(
                out,
                "  verdict: label={label:?} gap_delta={gap_delta:+.4} held_out_delta={held_out_delta:+.4}",
            );
            for warning in warnings {
                let _ = writeln!(
                    out,
                    "  WARNING [{}]: {}",
                    warning_kind(warning),
                    warning_detail(warning)
                );
            }
        }
        Verdict::Refused { kind, error } => {
            let _ = writeln!(out, "  REFUSED [{kind}]: {error}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_provenance_reduce_is_shared() {
        // The provenance reduce now lives in `commands::codeprobe`; r0b composes
        // it. A focused check that the shared rule still drives r0b's aggregation:
        // a None task makes the whole run gap:unavailable (AC3).
        let p =
            aggregate_provenance(&[HeldOutProvenance::External, HeldOutProvenance::None]).unwrap();
        assert_eq!(p, HeldOutProvenance::None);
    }
}
