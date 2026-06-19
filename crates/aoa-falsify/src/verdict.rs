use serde::{Deserialize, Serialize};

use crate::convention::ScoringConvention;
use crate::delta::repo_votes_for_proceed;
use crate::eligibility::is_eligible;
use crate::types::{FalsifyConfig, RepoResult};

/// The hardened R0/R0' verdict.
///
/// `Proceed` survives every R0' precondition; `Pivot` is the falsified outcome
/// (the migration is not the right layer); `Inconclusive` is the abstaining
/// outcome reached when a precondition cannot be satisfied. `Inconclusive` is
/// never silently converted to `Pivot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Proceed,
    Pivot,
    Inconclusive,
}

/// Minimum eligible-repo count R0 needs before a majority is meaningful.
const MIN_ELIGIBLE_REPOS: usize = 5;

/// Tally one run's eligible-repo votes under one convention into a verdict.
///
/// `Proceed` requires a strict majority of at least five eligible repos voting
/// that repo-delta >= harness-delta. An exact tie defaults to `Pivot`. A minority
/// is `Pivot`. Fewer than five eligible repos cannot establish a majority and is
/// `Inconclusive`. This is the base R0 tally before R0' hardening.
fn tally(repos: &[&RepoResult], run_index: usize, convention: &ScoringConvention) -> Verdict {
    if repos.len() < MIN_ELIGIBLE_REPOS {
        return Verdict::Inconclusive;
    }

    let mut votes_for = 0usize;
    for repo in repos {
        let tasks = &repo.runs[run_index].tasks;
        if repo_votes_for_proceed(tasks, convention) {
            votes_for += 1;
        }
    }

    let against = repos.len() - votes_for;
    if votes_for > against {
        Verdict::Proceed
    } else {
        // A strict-majority loss and an exact tie both default to pivot.
        Verdict::Pivot
    }
}

/// The outcome of running the full hardened pipeline, with the reasons captured
/// for the report.
#[derive(Debug, Clone)]
pub struct HardenedVerdict {
    pub verdict: Verdict,
    pub notes: Vec<String>,
}

/// Run R0 then apply every R0' precondition, downgrading a `Proceed` to
/// `Inconclusive` whenever a precondition fails. An `Inconclusive` produced by
/// any stage is preserved verbatim — it is never mapped onto `Pivot`.
///
/// Order: eligibility filter -> power precondition -> determinism across runs ->
/// convention-invariance. The canonical convention is run index 0's base tally.
pub fn decide(eligible: &[&RepoResult], config: &FalsifyConfig) -> HardenedVerdict {
    let mut notes = Vec::new();

    if eligible.len() < MIN_ELIGIBLE_REPOS {
        notes.push(format!(
            "only {} eligible repos; R0 needs >= {}",
            eligible.len(),
            MIN_ELIGIBLE_REPOS
        ));
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    if !power_satisfied(eligible, config, &mut notes) {
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    let canonical = ScoringConvention::canonical();

    let stable = determinism_satisfied(eligible, config, &canonical, &mut notes);
    let base = tally(eligible, 0, &canonical);

    if base == Verdict::Proceed && !stable {
        notes.push("verdict unstable across fixed-seed runs; downgraded to inconclusive".into());
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    if base == Verdict::Proceed && !convention_invariant(eligible, config, &mut notes) {
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    HardenedVerdict {
        verdict: base,
        notes,
    }
}

/// Whether the power precondition holds: every eligible repo meets the minimum
/// held-out size and the aggregate effect magnitude clears the threshold. Below
/// either, the evidence is too weak to assert a significant verdict (proceed or
/// pivot) and the gate abstains.
fn power_satisfied(
    eligible: &[&RepoResult],
    config: &FalsifyConfig,
    notes: &mut Vec<String>,
) -> bool {
    let canonical = ScoringConvention::canonical();

    for repo in eligible {
        if repo.holdout_size < config.min_holdout_size {
            notes.push(format!(
                "repo {} holdout {} below minimum {}; power precondition fails",
                repo.repo_id, repo.holdout_size, config.min_holdout_size
            ));
            return false;
        }
    }

    let mut effect_sum = 0.0;
    for repo in eligible {
        let d = crate::delta::repo_deltas(&repo.runs[0].tasks, &canonical);
        effect_sum += (d.repo_delta - d.harness_delta).abs();
    }
    let effect = effect_sum / eligible.len() as f64;
    if effect < config.min_effect_size {
        notes.push(format!(
            "aggregate effect size {effect:.4} below minimum {:.4}; power precondition fails",
            config.min_effect_size
        ));
        return false;
    }
    true
}

/// Whether the verdict is stable across the configured `k_runs` fixed-seed runs.
fn determinism_satisfied(
    eligible: &[&RepoResult],
    config: &FalsifyConfig,
    convention: &ScoringConvention,
    notes: &mut Vec<String>,
) -> bool {
    let k = config.k_runs as usize;
    let first = tally(eligible, 0, convention);
    for run_index in 1..k {
        if tally(eligible, run_index, convention) != first {
            notes.push(format!(
                "verdict differs at run {run_index} from run 0 across {k} fixed-seed runs"
            ));
            return false;
        }
    }
    notes.push(format!("verdict stable across {k} fixed-seed runs"));
    true
}

/// Whether a `Proceed` survives every admissible scoring convention. A flip away
/// from `Proceed` under any one fails the precondition.
fn convention_invariant(
    eligible: &[&RepoResult],
    config: &FalsifyConfig,
    notes: &mut Vec<String>,
) -> bool {
    for convention in &config.conventions {
        let v = tally(eligible, 0, convention);
        if v != Verdict::Proceed {
            notes.push(format!(
                "verdict flips to {:?} under convention '{}'; downgraded to inconclusive",
                v, convention.name
            ));
            return false;
        }
    }
    notes.push(format!(
        "verdict invariant across {} admissible conventions",
        config.conventions.len()
    ));
    true
}

/// Re-export the eligibility predicate at the verdict layer for the report's use.
pub fn partition(repos: &[RepoResult]) -> (Vec<&RepoResult>, Vec<&RepoResult>) {
    let mut eligible = Vec::new();
    let mut excluded = Vec::new();
    for repo in repos {
        if is_eligible(&repo.eligibility) {
            eligible.push(repo);
        } else {
            excluded.push(repo);
        }
    }
    (eligible, excluded)
}
