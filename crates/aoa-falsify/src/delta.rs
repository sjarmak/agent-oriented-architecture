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
/// deltas are zero (no evidence is not negative evidence).
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
pub fn repo_votes_for_proceed(tasks: &[PairTask], convention: &ScoringConvention) -> bool {
    let d = repo_deltas(tasks, convention);
    d.repo_delta >= d.harness_delta
}
