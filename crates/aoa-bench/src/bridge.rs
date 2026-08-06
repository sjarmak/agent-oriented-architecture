use std::collections::BTreeSet;

use aoa_domain::{RunResult, TaskOutcome};
use thiserror::Error;

use crate::task::CodeprobeTask;

/// The edit-locality anchors a codeprobe task supplies to `aoa-metrics`: the gold
/// artifact set `G_t` and two or more accepted-solution file-sets that define the
/// intersection floor and union ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditLocalityAnchors {
    pub gold_set: BTreeSet<String>,
    pub accepted_solutions: Vec<BTreeSet<String>>,
}

/// The one way supplying edit-locality anchors can fail: too few distinct
/// accepted solutions to form a floor and a ceiling.
///
/// Deliberately its own single-variant enum rather than a variant on the
/// task-loading [`crate::BenchError`]. Callers match it exhaustively so that a
/// second failure mode becomes a compile error here instead of being quietly
/// nulled into an "unavailable" row, and folding it into a twenty-variant enum
/// would take that away. It mirrors `aoa_metrics::MetricError` in shape and
/// message because both describe the same shortfall; it is a separate type
/// because an input-layer crate must not depend on a measurement crate to say
/// that a task was under-mined.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditLocalityError {
    /// Edit-locality requires at least two accepted solutions to form an
    /// intersection floor and a union ceiling.
    #[error("edit-locality needs >=2 accepted solutions, got {0}")]
    InsufficientAcceptedSolutions(usize),
}

impl CodeprobeTask {
    /// Gold artifact set `G_t` for retrieval-/edit-locality — the oracle's
    /// expected files anchored at mine time, not synthesized.
    pub fn gold_set(&self) -> &BTreeSet<String> {
        &self.oracle_files
    }

    /// Edit-locality anchors for this task, or
    /// [`EditLocalityError::InsufficientAcceptedSolutions`] when fewer than two
    /// accepted solutions were mined.
    ///
    /// The shortfall is reported with the count that fell short, exactly as
    /// `aoa_metrics::compute_edit_locality` reports it — a missing second
    /// solution is never fabricated to force a floor/ceiling.
    pub fn edit_locality_anchors(&self) -> Result<EditLocalityAnchors, EditLocalityError> {
        // Edit locality needs the spread between *distinct* accepted solutions:
        // two backends that mined identical files give no floor/ceiling
        // information, so they collapse here (they still count as independent
        // backends for provenance — that is a separate judgment).
        let distinct = self.accepted_solution_files();
        if distinct.len() < 2 {
            return Err(EditLocalityError::InsufficientAcceptedSolutions(
                distinct.len(),
            ));
        }
        Ok(EditLocalityAnchors {
            gold_set: self.oracle_files.clone(),
            accepted_solutions: distinct,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::AcceptedSolution;
    use aoa_domain::HeldOutProvenance;

    fn solution(backend: &str, files: &[&str]) -> AcceptedSolution {
        AcceptedSolution {
            backend: backend.to_string(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    fn task(accepted: Vec<AcceptedSolution>) -> CodeprobeTask {
        CodeprobeTask {
            id: "t".into(),
            repo: "r".into(),
            instruction: "i".into(),
            oracle_files: BTreeSet::from(["g.py".to_string()]),
            ground_truth_commit: None,
            accepted_solutions: accepted,
            oracle_chain: Default::default(),
        }
    }

    #[test]
    fn identical_backend_solutions_are_insufficient_for_edit_locality() {
        // The provenance/edit-locality split: two backends with identical files
        // are NativeComposed (provenance) but collapse to ONE distinct solution,
        // so edit locality fails loud rather than reporting a degenerate
        // floor==ceiling from a zero-width spread.
        let t = task(vec![
            solution("ast", &["a.py", "b.py"]),
            solution("treesitter", &["a.py", "b.py"]),
        ]);
        assert_eq!(t.held_out_provenance(), HeldOutProvenance::NativeComposed);
        match t.edit_locality_anchors() {
            Err(EditLocalityError::InsufficientAcceptedSolutions(n)) => assert_eq!(n, 1),
            other => panic!("expected InsufficientAcceptedSolutions(1), got {other:?}"),
        }
    }

    #[test]
    fn empty_file_backend_counts_for_provenance_but_not_edit_locality() {
        // A shipped backend with an empty file-set still counts as an independent
        // run (NativeComposed) but contributes no edit-locality anchor.
        let t = task(vec![
            solution("ast", &["a.py", "b.py"]),
            solution("treesitter", &[]),
        ]);
        assert_eq!(t.held_out_provenance(), HeldOutProvenance::NativeComposed);
        // Only one non-empty distinct solution survives for edit locality.
        assert_eq!(t.accepted_solution_files().len(), 1);
        assert!(matches!(
            t.edit_locality_anchors(),
            Err(EditLocalityError::InsufficientAcceptedSolutions(1))
        ));
    }
}
