use serde::{Deserialize, Serialize};

use crate::convention::ScoringConvention;
use crate::delta::repo_deltas;
use crate::error::FalsifyError;
use crate::types::FalsifyInput;
use crate::verdict::{decide, partition, Verdict};

/// The serializable `falsification.json` payload.
///
/// `repo_delta` and `harness_delta` are the mean held-out success deltas over
/// eligible repos under the canonical convention, for transparency; the
/// `verdict` is the hardened R0/R0' outcome. The eligible and excluded repo ids,
/// the conventions actually evaluated, and the precondition notes are emitted so
/// the decision is fully inspectable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FalsifyReport {
    /// Mean held-out success on the repo arm (AOA migration) over eligible repos.
    pub repo_delta: f64,
    /// Mean held-out success on the harness arm (swapped harness) over eligible
    /// repos.
    pub harness_delta: f64,
    pub verdict: Verdict,
    pub eligible_repos: Vec<String>,
    pub excluded_repos: Vec<String>,
    /// The admissible conventions the verdict was checked against, as data.
    pub conventions_tried: Vec<String>,
    pub notes: Vec<String>,
}

impl FalsifyReport {
    /// Serialize to the `falsification.json` string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Run the wrong-layer falsification gate (R0) with R0' hardening.
///
/// Computes the per-repo repo-arm and harness-arm held-out success over
/// identical-pair tasks, then decides a hardened verdict in
/// `{proceed, pivot, inconclusive}`. Errors only on structurally invalid input
/// (fewer than five repos, missing run snapshots, too few replications); a
/// downgrade to `inconclusive` is a data outcome, not an error.
pub fn falsify(input: &FalsifyInput) -> Result<FalsifyReport, FalsifyError> {
    if input.repos.len() < 5 {
        return Err(FalsifyError::TooFewRepos(input.repos.len()));
    }
    if input.config.k_runs < 3 {
        return Err(FalsifyError::InsufficientReplication(input.config.k_runs));
    }
    let needed = input.config.k_runs as usize;
    for repo in &input.repos {
        if repo.runs.len() < needed {
            return Err(FalsifyError::EmptyRuns {
                repo_id: repo.repo_id.clone(),
            });
        }
    }

    let (eligible, excluded) = partition(&input.repos);

    let canonical = ScoringConvention::canonical();
    let (mut repo_sum, mut harness_sum) = (0.0, 0.0);
    for repo in &eligible {
        let d = repo_deltas(&repo.runs[0].tasks, &canonical);
        repo_sum += d.repo_delta;
        harness_sum += d.harness_delta;
    }
    let n = eligible.len().max(1) as f64;

    let hardened = decide(&eligible, &input.config);

    Ok(FalsifyReport {
        repo_delta: repo_sum / n,
        harness_delta: harness_sum / n,
        verdict: hardened.verdict,
        eligible_repos: eligible.iter().map(|r| r.repo_id.clone()).collect(),
        excluded_repos: excluded.iter().map(|r| r.repo_id.clone()).collect(),
        conventions_tried: input
            .config
            .conventions
            .iter()
            .map(|c| c.name.clone())
            .collect(),
        notes: hardened.notes,
    })
}
