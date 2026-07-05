use serde::{Deserialize, Serialize};

use crate::convention::{ConventionFamily, ScoringConvention};
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

/// One run's tally under one convention: the verdict plus the repos that
/// admitted zero pairs (and therefore cast no vote).
struct TallyOutcome {
    verdict: Verdict,
    /// Repos whose tasks the convention admitted none of. Any such repo makes
    /// the tally `Inconclusive`: a convention that excludes a repo's entire
    /// evidence cannot be counted for (or against) invariance.
    zero_admission_repos: Vec<String>,
}

/// Tally one run's eligible-repo votes under one convention into a verdict.
///
/// `Proceed` requires a strict majority of at least five eligible repos voting
/// that repo-delta >= harness-delta. An exact tie defaults to `Pivot`. A minority
/// is `Pivot`. Fewer than five eligible repos cannot establish a majority and is
/// `Inconclusive`, as is any repo with zero admitted pairs (no evidence is not a
/// vote). This is the base R0 tally before R0' hardening.
fn tally(repos: &[&RepoResult], run_index: usize, convention: &ScoringConvention) -> TallyOutcome {
    if repos.len() < MIN_ELIGIBLE_REPOS {
        return TallyOutcome {
            verdict: Verdict::Inconclusive,
            zero_admission_repos: Vec::new(),
        };
    }

    let mut votes_for = 0usize;
    let mut zero_admission_repos = Vec::new();
    for repo in repos {
        let tasks = &repo.runs[run_index].tasks;
        match repo_votes_for_proceed(tasks, convention) {
            Some(true) => votes_for += 1,
            Some(false) => {}
            None => zero_admission_repos.push(repo.repo_id.clone()),
        }
    }
    if !zero_admission_repos.is_empty() {
        return TallyOutcome {
            verdict: Verdict::Inconclusive,
            zero_admission_repos,
        };
    }

    let against = repos.len() - votes_for;
    let verdict = if votes_for > against {
        Verdict::Proceed
    } else {
        // A strict-majority loss and an exact tie both default to pivot.
        Verdict::Pivot
    };
    TallyOutcome {
        verdict,
        zero_admission_repos,
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
/// `family` is the (validated, uniform) family of the tasks' convention inputs;
/// the canonical convention is built for it so every task is admitted.
///
/// Order: eligibility filter -> power precondition -> determinism across runs ->
/// convention-invariance. The canonical convention is run index 0's base tally.
pub fn decide(
    eligible: &[&RepoResult],
    config: &FalsifyConfig,
    family: ConventionFamily,
) -> HardenedVerdict {
    let mut notes = Vec::new();
    let canonical = ScoringConvention::canonical(family);

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

    if !power_satisfied(eligible, config, &canonical, &mut notes) {
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    let base = tally(eligible, 0, &canonical);
    if !base.zero_admission_repos.is_empty() {
        notes.push(format!(
            "repo(s) {} carry zero identical-pair tasks under the canonical convention; \
             a repo with no admitted evidence cannot vote — verdict inconclusive",
            base.zero_admission_repos.join(", ")
        ));
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    let stable = determinism_satisfied(eligible, config, &canonical, &mut notes);

    if base.verdict == Verdict::Proceed && !stable {
        notes.push("verdict unstable across fixed-seed runs; downgraded to inconclusive".into());
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    if base.verdict == Verdict::Proceed && !convention_invariant(eligible, config, &mut notes) {
        return HardenedVerdict {
            verdict: Verdict::Inconclusive,
            notes,
        };
    }

    HardenedVerdict {
        verdict: base.verdict,
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
    canonical: &ScoringConvention,
    notes: &mut Vec<String>,
) -> bool {
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
        let d = crate::delta::repo_deltas(&repo.runs[0].tasks, canonical);
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
    let first = tally(eligible, 0, convention).verdict;
    for run_index in 1..k {
        if tally(eligible, run_index, convention).verdict != first {
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
/// from `Proceed` under any one fails the precondition — and a convention that
/// admits zero pairs for any repo fails it too: an all-excluding convention is
/// unexercised evidence, never vacuous invariance.
fn convention_invariant(
    eligible: &[&RepoResult],
    config: &FalsifyConfig,
    notes: &mut Vec<String>,
) -> bool {
    for convention in &config.conventions {
        let outcome = tally(eligible, 0, convention);
        if !outcome.zero_admission_repos.is_empty() {
            notes.push(format!(
                "convention '{}' admits zero identical-pair tasks for repo(s) {}; \
                 invariance cannot be exercised on no evidence — downgraded to inconclusive",
                convention.name,
                outcome.zero_admission_repos.join(", ")
            ));
            return false;
        }
        if outcome.verdict != Verdict::Proceed {
            notes.push(format!(
                "verdict flips to {:?} under convention '{}'; downgraded to inconclusive",
                outcome.verdict, convention.name
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
