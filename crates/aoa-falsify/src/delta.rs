use crate::convention::ScoringConvention;
use crate::types::PairTask;

/// A repo's two held-out success deltas over identical-pair tasks, computed
/// under one scoring convention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepoDeltas {
    /// Weighted held-out success rate on the repo arm (AOA migration).
    pub repo_delta: f64,
    /// Weighted held-out success rate on the harness arm (swapped harness).
    pub harness_delta: f64,
    /// Number of identical-pair tasks admitted under the convention.
    pub admitted: usize,
}

/// Compute a repo's held-out success deltas over its identical-pair tasks under
/// one convention.
///
/// Only identical-pair tasks the convention admits contribute; non-paired tasks
/// and tasks excluded by the convention are dropped. With no admitted tasks both
/// deltas are zero and `admitted` is zero — callers MUST read `admitted` and
/// treat that as absent evidence (see [`repo_votes_for_proceed`]), never as a
/// `0.0 >= 0.0` tie that favors proceed.
pub fn repo_deltas(tasks: &[PairTask], convention: &ScoringConvention) -> RepoDeltas {
    let admitted: Vec<&PairTask> = tasks
        .iter()
        .filter(|t| t.is_identical_pair && convention.admits(t))
        .collect();

    if admitted.is_empty() {
        return RepoDeltas {
            repo_delta: 0.0,
            harness_delta: 0.0,
            admitted: 0,
        };
    }

    let n = admitted.len() as f64;
    let repo_hits = admitted.iter().filter(|t| t.repo_held_out_success).count() as f64;
    let harness_hits = admitted
        .iter()
        .filter(|t| t.harness_held_out_success)
        .count() as f64;

    RepoDeltas {
        repo_delta: convention.repo_weight * repo_hits / n,
        harness_delta: convention.harness_weight * harness_hits / n,
        admitted: admitted.len(),
    }
}

/// Whether a repo votes "repo arm wins" under one convention: its repo-delta is
/// at least its harness-delta on its admitted identical-pair tasks.
///
/// `None` when the convention admits zero of the repo's pairs: a repo with no
/// admitted evidence casts NO vote. (Comparing the zero deltas would read
/// `0.0 >= 0.0` as a proceed vote, letting a convention that excludes
/// everything pass R0' convention-invariance vacuously.)
pub fn repo_votes_for_proceed(tasks: &[PairTask], convention: &ScoringConvention) -> Option<bool> {
    let d = repo_deltas(tasks, convention);
    if d.admitted == 0 {
        return None;
    }
    Some(d.repo_delta >= d.harness_delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ConventionInputs;

    fn answer_pair(harness_depth: u32) -> PairTask {
        PairTask {
            task_id: 1,
            is_identical_pair: true,
            repo_held_out_success: true,
            harness_held_out_success: false,
            convention_inputs: ConventionInputs::Answer {
                repo_trace_locality: 1.0,
                harness_trace_locality: 1.0,
                repo_trace_reach_depth: 0,
                harness_trace_reach_depth: harness_depth,
            },
        }
    }

    /// Zero admitted pairs is absent evidence (no vote), never a proceed vote.
    #[test]
    fn zero_admitted_pairs_casts_no_vote() {
        let depth_k = ScoringConvention::admissible_answer()
            .into_iter()
            .find(|c| c.name == "trace_reach_depth_k")
            .unwrap();

        // The single pair saturates harness reach: depth-k admits nothing.
        let tasks = vec![answer_pair(u32::MAX)];
        assert_eq!(repo_deltas(&tasks, &depth_k).admitted, 0);
        assert_eq!(repo_votes_for_proceed(&tasks, &depth_k), None);

        // The same pair within the bound is admitted and votes.
        let tasks = vec![answer_pair(0)];
        assert_eq!(repo_votes_for_proceed(&tasks, &depth_k), Some(true));
    }
}
