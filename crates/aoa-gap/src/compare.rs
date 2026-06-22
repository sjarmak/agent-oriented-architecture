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

/// A non-fatal advisory surfaced alongside a computed comparison.
///
/// A warning does not flip the label or fail the gate; it names a condition the
/// gate could not adjudicate so the operator sees it rather than relying on a
/// silent pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareWarning {
    /// The leakage signature (held-out rose while visible stayed flat) is present
    /// but neither run carried a canary, so the leakage check had no known-item
    /// signal to confirm or clear it. The comparison proceeded — leakage cannot be
    /// proven without a canary — but the leak-shaped signal is reported loudly
    /// rather than passing silently.
    ZeroCanaryLeakShape,
}

/// The result of comparing a baseline run against a migrated run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareOutcome {
    /// `migrated.gap - baseline.gap`; `<= 0` means the gap held or shrank.
    pub gap_delta: f64,
    /// `migrated.held_out_rate - baseline.held_out_rate`; `> 0` means improvement.
    pub held_out_delta: f64,
    pub label: Label,
    /// Non-fatal advisories the comparison could not adjudicate; empty in the
    /// common clean case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<CompareWarning>,
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

/// Whether the held-out rate rose while the visible leg stayed flat — the
/// leakage signature on its own, independent of any canary. Confirming a leak
/// still requires a flipped known-held-out canary (see [`leakage_detected`]);
/// this predicate isolates the shape so it can both feed that confirmation and
/// drive a zero-canary warning when no canary exists to adjudicate it.
///
/// "Flat" tolerates one task's worth of movement (`1/N`) rather than demanding
/// an exact-equal visible rate: a real leak that also nudges the visible leg by
/// a single task out of N would otherwise read as not-flat and fail open. The
/// band uses the smaller task count so a one-task flip in either run is covered;
/// for the common same-task-set case both counts are N. A broad gain that lifts
/// visible well beyond one task is honest capability, not a held-out-specific
/// leak, and is deliberately left outside the band.
fn leak_shaped(baseline: &RunResult, migrated: &RunResult) -> bool {
    let held_out_rose = migrated.held_out_rate() > baseline.held_out_rate();
    let n = baseline.tasks.len().min(migrated.tasks.len());
    let visible_tol = if n == 0 { 0.0 } else { 1.0 / n as f64 };
    let visible_flat = (migrated.visible_rate() - baseline.visible_rate()).abs() <= visible_tol;
    held_out_rose && visible_flat
}

/// Whether the leakage canary trips: held-out rose while the visible leg stayed
/// flat and an injected canary flipped against its expected outcome. Held-out
/// improving without matching visible movement is the leakage signature; a
/// flipped known-held-out canary confirms the suite was contaminated.
fn leakage_detected(baseline: &RunResult, migrated: &RunResult) -> bool {
    let canary_flipped = baseline.any_canary_flipped() || migrated.any_canary_flipped();
    leak_shaped(baseline, migrated) && canary_flipped
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

    // The leak-shaped signal with no canary on either run cannot be confirmed as
    // leakage, but it must not pass silently: warn loudly so the absence of a
    // canary on a contamination-prone comparison is visible rather than implied.
    let mut warnings = Vec::new();
    let no_canaries = baseline.canaries.is_empty() && migrated.canaries.is_empty();
    if no_canaries && leak_shaped(baseline, migrated) {
        warnings.push(CompareWarning::ZeroCanaryLeakShape);
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
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::HeldOutProvenance;
    use crate::run::{CanaryItem, TaskOutcome};

    /// Build a `NativeComposed` run from per-task `(visible, held_out)` pairs and
    /// optional canaries. `NativeComposed` keeps `compute_gap` in the `Available`
    /// arm so the comparison reaches the leakage/warning logic.
    fn run(tasks: &[(bool, bool)], canaries: Vec<CanaryItem>) -> RunResult {
        RunResult {
            tasks: tasks
                .iter()
                .map(|&(visible_success, held_out_success)| TaskOutcome {
                    visible_success,
                    held_out_success,
                })
                .collect(),
            held_out_provenance: HeldOutProvenance::NativeComposed,
            canaries,
        }
    }

    fn canary(flipped: bool) -> CanaryItem {
        // expected clean = pass; an observed fail is the flip.
        CanaryItem {
            id: "c0".to_string(),
            held_out_success: !flipped,
            expected_held_out: true,
        }
    }

    /// The leak signature with NO canary on either run is reported as a warning
    /// rather than passing silently — the d6t.7 regression.
    #[test]
    fn zero_canary_leak_shape_warns() {
        // baseline: visible high, held-out low; migrated: held-out rose, visible flat.
        let baseline = run(&[(true, false), (true, false)], vec![]);
        let migrated = run(&[(true, true), (true, false)], vec![]);

        let outcome = compare(&baseline, &migrated).expect("leak shape alone must not refuse");
        assert!(
            outcome
                .warnings
                .contains(&CompareWarning::ZeroCanaryLeakShape),
            "zero-canary leak shape must warn, got {:?}",
            outcome.warnings
        );
    }

    /// The same leak signature WITH a flipped canary still fails the gate loudly;
    /// the warning path must not have weakened the refusal.
    #[test]
    fn leak_shape_with_flipped_canary_still_refuses() {
        let baseline = run(&[(true, false), (true, false)], vec![canary(false)]);
        let migrated = run(&[(true, true), (true, false)], vec![canary(true)]);

        let err = compare(&baseline, &migrated).expect_err("flipped canary must refuse");
        assert!(matches!(err, GapError::LeakageDetected));
    }

    /// Leak shape with canaries present but none flipped: the operator injected
    /// canaries and the gate cleared the run — no warning noise.
    #[test]
    fn leak_shape_with_unflipped_canary_does_not_warn() {
        let baseline = run(&[(true, false), (true, false)], vec![canary(false)]);
        let migrated = run(&[(true, true), (true, false)], vec![canary(false)]);

        let outcome = compare(&baseline, &migrated).expect("unflipped canary must not refuse");
        assert!(
            outcome.warnings.is_empty(),
            "canary present and clean must not warn, got {:?}",
            outcome.warnings
        );
    }

    /// No leak shape (held-out did not rise) and zero canaries: clean, no warning.
    #[test]
    fn no_leak_shape_no_warning() {
        let baseline = run(&[(true, true), (true, true)], vec![]);
        let migrated = run(&[(true, true), (true, true)], vec![]);

        let outcome = compare(&baseline, &migrated).expect("clean comparison");
        assert!(outcome.warnings.is_empty());
    }
}
