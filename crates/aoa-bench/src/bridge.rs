use std::collections::BTreeSet;

use aoa_gap::{RunResult, TaskOutcome};
use aoa_metrics::MetricError;

use crate::task::CodeprobeTask;

/// The edit-locality anchors a codeprobe task supplies to `aoa-metrics`: the gold
/// artifact set `G_t` and two or more accepted-solution file-sets that define the
/// intersection floor and union ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditLocalityAnchors {
    pub gold_set: BTreeSet<String>,
    pub accepted_solutions: Vec<BTreeSet<String>>,
}

impl CodeprobeTask {
    /// Gold artifact set `G_t` for retrieval-/edit-locality — the oracle's
    /// expected files anchored at mine time, not synthesized.
    pub fn gold_set(&self) -> &BTreeSet<String> {
        &self.oracle_files
    }

    /// Edit-locality anchors for this task, or `InsufficientAcceptedSolutions`
    /// when fewer than two accepted solutions were mined.
    ///
    /// The shortfall is surfaced as the same `aoa-metrics` error
    /// `compute_edit_locality` would raise — a missing second solution is never
    /// fabricated to force a floor/ceiling.
    pub fn edit_locality_anchors(&self) -> Result<EditLocalityAnchors, MetricError> {
        if self.accepted_solutions.len() < 2 {
            return Err(MetricError::InsufficientAcceptedSolutions(
                self.accepted_solutions.len(),
            ));
        }
        Ok(EditLocalityAnchors {
            gold_set: self.oracle_files.clone(),
            accepted_solutions: self.accepted_solutions.clone(),
        })
    }

    /// Build the `aoa-gap` run input for a single observed outcome, stamped with
    /// this task's classified held-out provenance.
    ///
    /// When the provenance is `None`, `aoa_gap::compute_gap` yields
    /// `GapOutcome::Unavailable` (gap:unavailable) — no held-out suite is invented.
    pub fn to_run_result(&self, visible_success: bool, held_out_success: bool) -> RunResult {
        RunResult {
            tasks: vec![TaskOutcome {
                visible_success,
                held_out_success,
            }],
            held_out_provenance: self.held_out_provenance(),
            canaries: Vec::new(),
        }
    }
}
