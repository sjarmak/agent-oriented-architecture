use serde::{Deserialize, Serialize};

use crate::convention::{ConventionFamily, ScoringConvention};
use crate::delta::repo_deltas;
use crate::error::FalsifyError;
use crate::types::FalsifyInput;
use crate::verdict::{decide, partition, Verdict};

/// The serializable `falsification.json` payload.
///
/// `repo_delta` and `harness_delta` are the mean held-out success deltas over
/// eligible repos under the canonical convention, for transparency; the
/// `verdict` is the hardened R0/R0' outcome. The eligible and excluded repo ids,
/// the convention family and the conventions actually evaluated, and the
/// precondition notes are emitted so the decision is fully inspectable.
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
    /// The task-shape family of the convention inputs the verdict was computed
    /// over — `edit` (SDLC tasks) or `answer` (comprehension tasks).
    pub convention_family: ConventionFamily,
    /// The admissible conventions the verdict was checked against, with FULL
    /// parameters (thresholds, depths, weights) — names alone cannot show a
    /// tampered set, so the whole pre-registered data is emitted.
    pub conventions_tried: Vec<ScoringConvention>,
    pub notes: Vec<String>,
}

impl FalsifyReport {
    /// Serialize to the `falsification.json` string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Derive the single task-shape family of the input and validate the
/// configured conventions against it.
///
/// Every task's `convention_inputs` must carry the same family, and every
/// configured convention must score that family — otherwise scoring would
/// silently admit nothing (or the wrong things), so both are input errors. An
/// input with no tasks at all defaults to `Edit`; no admission decision is ever
/// taken on it.
///
/// Beyond the family, the configured set must structurally EQUAL the
/// pre-registered admissible set for that family
/// ([`ScoringConvention::admissible_edit`] / [`admissible_answer`]
/// (ScoringConvention::admissible_answer)): a hand-edited threshold, depth, or
/// weight hides behind an unchanged name, so nothing but the exact
/// pre-registered data is accepted. There is deliberately no override flag.
fn validated_family(input: &FalsifyInput) -> Result<ConventionFamily, FalsifyError> {
    let mut family: Option<ConventionFamily> = None;
    for repo in &input.repos {
        for run in &repo.runs {
            for task in &run.tasks {
                let f = task.convention_inputs.family();
                match family {
                    None => family = Some(f),
                    Some(existing) if existing != f => {
                        return Err(FalsifyError::MixedInputFamilies)
                    }
                    Some(_) => {}
                }
            }
        }
    }
    let family = family.unwrap_or(ConventionFamily::Edit);
    for convention in &input.config.conventions {
        if convention.family != family {
            return Err(FalsifyError::ConventionFamilyMismatch {
                name: convention.name.clone(),
                convention: convention.family,
                input: family,
            });
        }
    }
    let admissible = match family {
        ConventionFamily::Edit => ScoringConvention::admissible_edit(),
        ConventionFamily::Answer => ScoringConvention::admissible_answer(),
    };
    if input.config.conventions != admissible {
        return Err(FalsifyError::ConventionSetNotPreRegistered { family });
    }
    Ok(family)
}

/// Run the wrong-layer falsification gate (R0) with R0' hardening.
///
/// Computes the per-repo repo-arm and harness-arm held-out success over
/// identical-pair tasks, then decides a hardened verdict in
/// `{proceed, pivot, inconclusive}`. Errors only on structurally invalid input
/// (fewer than five repos, missing run snapshots, too few replications, mixed
/// or mismatched convention-input families); a downgrade to `inconclusive` is a
/// data outcome, not an error.
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
    let family = validated_family(input)?;

    let (eligible, excluded) = partition(&input.repos);

    let canonical = ScoringConvention::canonical(family);
    let (mut repo_sum, mut harness_sum) = (0.0, 0.0);
    for repo in &eligible {
        let d = repo_deltas(&repo.runs[0].tasks, &canonical);
        repo_sum += d.repo_delta;
        harness_sum += d.harness_delta;
    }
    let n = eligible.len().max(1) as f64;

    let hardened = decide(&eligible, &input.config, family);

    Ok(FalsifyReport {
        repo_delta: repo_sum / n,
        harness_delta: harness_sum / n,
        verdict: hardened.verdict,
        eligible_repos: eligible.iter().map(|r| r.repo_id.clone()).collect(),
        excluded_repos: excluded.iter().map(|r| r.repo_id.clone()).collect(),
        convention_family: family,
        conventions_tried: input.config.conventions.clone(),
        notes: hardened.notes,
    })
}
