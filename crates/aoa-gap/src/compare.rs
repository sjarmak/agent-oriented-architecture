use serde::{Deserialize, Serialize};

use crate::error::GapError;
use crate::gap::{compute_gap, GapOutcome};
use crate::run::RunResult;

/// Whether a migration earns the `good` label.
///
/// `Good` requires the held-out pass rate to improve AND the gap to hold or
/// reduce. A visible-pass + locality-only improvement does not move held-out
/// and is therefore `NotGood`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Good,
    NotGood,
}

/// The result of comparing a baseline run against a migrated run.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CompareOutcome {
    /// `migrated.gap - baseline.gap`; `<= 0` means the gap held or shrank.
    pub gap_delta: f64,
    /// `migrated.held_out_rate - baseline.held_out_rate`; `> 0` means improvement.
    pub held_out_delta: f64,
    pub label: Label,
}

/// Per-run figures pulled out of an `Available` gap outcome for comparison.
struct RunFigures {
    held_out_rate: f64,
    gap: f64,
}

/// Resolve a run to its computed figures, refusing to proceed on an absent gap.
fn figures(run: &RunResult) -> Result<RunFigures, GapError> {
    match compute_gap(run)? {
        GapOutcome::Available {
            held_out_rate, gap, ..
        } => Ok(RunFigures { held_out_rate, gap }),
        GapOutcome::Unavailable => Err(GapError::GapUnavailable),
    }
}

/// Whether the leakage canary trips: held-out rose while the visible leg stayed
/// flat and an injected canary flipped against its expected outcome. Held-out
/// improving without matching visible movement is the leakage signature; a
/// flipped known-held-out canary confirms the suite was contaminated.
///
/// "Flat" tolerates one task's worth of movement (`1/N`) rather than demanding
/// an exact-equal visible rate: a real leak that also nudges the visible leg by
/// a single task out of N would otherwise read as not-flat and fail open. The
/// band uses the smaller task count so a one-task flip in either run is covered;
/// for the common same-task-set case both counts are N. A broad gain that lifts
/// visible well beyond one task is honest capability, not a held-out-specific
/// leak, and is deliberately left outside the band.
fn leakage_detected(baseline: &RunResult, migrated: &RunResult) -> bool {
    let held_out_rose = migrated.held_out_rate() > baseline.held_out_rate();
    let n = baseline.tasks.len().min(migrated.tasks.len());
    let visible_tol = if n == 0 { 0.0 } else { 1.0 / n as f64 };
    let visible_flat = (migrated.visible_rate() - baseline.visible_rate()).abs() <= visible_tol;
    let canary_flipped = baseline.any_canary_flipped() || migrated.any_canary_flipped();
    held_out_rose && visible_flat && canary_flipped
}

/// Compare a baseline run against a migrated run and decide whether the
/// migration is `good`.
///
/// `--compare baseline migrated` semantics. Synthesized held-out suites are
/// rejected, an absent gap refuses to label, and a tripped leakage canary fails
/// the comparison.
pub fn compare(baseline: &RunResult, migrated: &RunResult) -> Result<CompareOutcome, GapError> {
    let base = figures(baseline)?;
    let migr = figures(migrated)?;

    if leakage_detected(baseline, migrated) {
        return Err(GapError::LeakageDetected);
    }

    let gap_delta = migr.gap - base.gap;
    let held_out_delta = migr.held_out_rate - base.held_out_rate;

    let label = if held_out_delta > 0.0 && gap_delta <= 0.0 {
        Label::Good
    } else {
        Label::NotGood
    };

    Ok(CompareOutcome {
        gap_delta,
        held_out_delta,
        label,
    })
}
