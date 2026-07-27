use serde::{Deserialize, Serialize};

use crate::types::{ConventionInputs, PairTask};

/// Which task shape a convention scores.
///
/// The two families read *different* per-task inputs ([`ConventionInputs`]):
/// `Edit` conventions bound the edit-locality/mutation-depth of an edit-shaped
/// SDLC task (derived from >=2 accepted PR solutions); `Answer` conventions
/// bound the trace-locality/trace-reach-depth of an answer-shaped comprehension
/// task (derived from the trial trace joined to the task's oracle chain over
/// the SCIP symbol graph). A convention never admits a task of the other
/// family, and `falsify` rejects a mixed input outright — the family actually
/// exercised is emitted in the report so it is never ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConventionFamily {
    Edit,
    Answer,
}

/// Which locality bound a convention applies when admitting a task.
///
/// `Floor` admits only tasks at or above a locality floor; `Ceiling` admits only
/// tasks at or below a locality ceiling. The two are the extremes of the
/// admissible scoring space — a `proceed` must survive both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalityBound {
    Floor,
    Ceiling,
}

/// One admissible scoring convention. A `proceed` verdict must be invariant
/// across every convention in the admissible set; a flip under any one downgrades
/// the verdict to `inconclusive`.
///
/// Conventions are data, not hidden code paths: the set actually evaluated is
/// emitted in the report so the precondition is inspectable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoringConvention {
    /// Human-readable label, emitted in the report.
    pub name: String,
    /// The task shape this convention scores. Must match the family of every
    /// task's [`ConventionInputs`]; `falsify` errors on a mismatch.
    pub family: ConventionFamily,
    /// Which locality bound to apply (edit-locality for `Edit`, trace-locality
    /// for `Answer`).
    pub locality_bound: LocalityBound,
    /// The locality threshold the bound is measured against.
    pub locality_threshold: f64,
    /// Maximum admissible depth: mutation-surface depth for `Edit` tasks,
    /// trace-reach depth for `Answer` tasks. Tasks deeper than this are
    /// excluded under this convention.
    pub max_depth: u32,
    /// Weight applied to a repo-arm success when counting held-out success.
    pub repo_weight: f64,
    /// Weight applied to a harness-arm success when counting held-out success.
    pub harness_weight: f64,
}

impl ScoringConvention {
    /// Whether a task is admitted for scoring under this convention.
    ///
    /// A task whose inputs belong to the other family is never admitted (and
    /// `falsify` rejects such an input before scoring, so this arm is a
    /// defense-in-depth guard, not a routine path). Answer-task bounds apply to
    /// BOTH arms' trial inputs: a pair participates only when the repo-arm and
    /// harness-arm trials each satisfy the bound.
    pub fn admits(&self, task: &PairTask) -> bool {
        match (self.family, &task.convention_inputs) {
            (
                ConventionFamily::Edit,
                ConventionInputs::Edit {
                    edit_locality,
                    mutation_depth,
                },
            ) => {
                if *mutation_depth > self.max_depth {
                    return false;
                }
                self.locality_within_bound(*edit_locality)
            }
            (
                ConventionFamily::Answer,
                ConventionInputs::Answer {
                    repo_trace_locality,
                    harness_trace_locality,
                    repo_trace_reach_depth,
                    harness_trace_reach_depth,
                },
            ) => {
                if *repo_trace_reach_depth > self.max_depth
                    || *harness_trace_reach_depth > self.max_depth
                {
                    return false;
                }
                self.locality_within_bound(*repo_trace_locality)
                    && self.locality_within_bound(*harness_trace_locality)
            }
            _ => false,
        }
    }

    fn locality_within_bound(&self, locality: f64) -> bool {
        match self.locality_bound {
            LocalityBound::Floor => locality >= self.locality_threshold,
            LocalityBound::Ceiling => locality <= self.locality_threshold,
        }
    }

    /// The admissible set for edit-shaped SDLC tasks: edit-locality floor and
    /// ceiling, a deeper mutation-surface depth-k, and an alternative
    /// metric-weighting. All of these are extremes a real `proceed` must survive.
    pub fn admissible_edit() -> Vec<Self> {
        vec![
            Self {
                name: "edit_locality_floor".to_string(),
                family: ConventionFamily::Edit,
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_depth: u32::MAX,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "edit_locality_ceiling".to_string(),
                family: ConventionFamily::Edit,
                locality_bound: LocalityBound::Ceiling,
                locality_threshold: 1.0,
                max_depth: u32::MAX,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "mutation_surface_depth_k".to_string(),
                family: ConventionFamily::Edit,
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_depth: 3,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "alternative_metric_weights".to_string(),
                family: ConventionFamily::Edit,
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_depth: u32::MAX,
                repo_weight: 0.75,
                harness_weight: 1.25,
            },
        ]
    }

    /// The admissible set for answer-shaped comprehension tasks, pre-registered
    /// 2026-07-04 (bead aoa-dhk.1) BEFORE any campaign data existed — see
    /// `docs/r0_runbook.md` § "Answer-task convention set".
    ///
    /// Structurally the exact analogue of [`Self::admissible_edit`]:
    /// trace-locality floor and ceiling at the extremes of the `[0, 1]` locality
    /// space, a trace-reach depth-k bound at 3, and the same alternative
    /// metric-weighting. Bounds apply to both arms' trial inputs.
    pub fn admissible_answer() -> Vec<Self> {
        vec![
            Self {
                name: "trace_locality_floor".to_string(),
                family: ConventionFamily::Answer,
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_depth: u32::MAX,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "trace_locality_ceiling".to_string(),
                family: ConventionFamily::Answer,
                locality_bound: LocalityBound::Ceiling,
                locality_threshold: 1.0,
                max_depth: u32::MAX,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "trace_reach_depth_k".to_string(),
                family: ConventionFamily::Answer,
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_depth: 3,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "alternative_metric_weights".to_string(),
                family: ConventionFamily::Answer,
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_depth: u32::MAX,
                repo_weight: 0.75,
                harness_weight: 1.25,
            },
        ]
    }

    /// The canonical convention used for the report's emitted deltas: no task
    /// exclusion (within the given family) and equal weights.
    pub fn canonical(family: ConventionFamily) -> Self {
        Self {
            name: "canonical".to_string(),
            family,
            locality_bound: LocalityBound::Floor,
            locality_threshold: 0.0,
            max_depth: u32::MAX,
            repo_weight: 1.0,
            harness_weight: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer_task(
        repo_loc: f64,
        harness_loc: f64,
        repo_depth: u32,
        harness_depth: u32,
    ) -> PairTask {
        PairTask {
            task_id: 0,
            is_identical_pair: true,
            repo_held_out_success: true,
            harness_held_out_success: false,
            convention_inputs: ConventionInputs::Answer {
                repo_trace_locality: repo_loc,
                harness_trace_locality: harness_loc,
                repo_trace_reach_depth: repo_depth,
                harness_trace_reach_depth: harness_depth,
            },
        }
    }

    fn edit_task(locality: f64, depth: u32) -> PairTask {
        PairTask {
            task_id: 0,
            is_identical_pair: true,
            repo_held_out_success: true,
            harness_held_out_success: false,
            convention_inputs: ConventionInputs::Edit {
                edit_locality: locality,
                mutation_depth: depth,
            },
        }
    }

    #[test]
    fn answer_depth_bound_applies_to_both_arms() {
        let depth_k = ScoringConvention::admissible_answer()
            .into_iter()
            .find(|c| c.name == "trace_reach_depth_k")
            .unwrap();
        // Both arms within depth 3: admitted.
        assert!(depth_k.admits(&answer_task(1.0, 1.0, 0, 3)));
        // Either arm beyond depth 3 (including the unreachable saturation):
        // excluded.
        assert!(!depth_k.admits(&answer_task(1.0, 1.0, 4, 0)));
        assert!(!depth_k.admits(&answer_task(1.0, 1.0, 0, u32::MAX)));
    }

    #[test]
    fn answer_locality_floor_and_ceiling_bound_both_arms() {
        let floor_half = ScoringConvention {
            name: "floor_half".to_string(),
            family: ConventionFamily::Answer,
            locality_bound: LocalityBound::Floor,
            locality_threshold: 0.5,
            max_depth: u32::MAX,
            repo_weight: 1.0,
            harness_weight: 1.0,
        };
        assert!(floor_half.admits(&answer_task(0.5, 0.9, 0, 0)));
        assert!(!floor_half.admits(&answer_task(0.9, 0.4, 0, 0)));

        let ceiling_half = ScoringConvention {
            locality_bound: LocalityBound::Ceiling,
            ..floor_half
        };
        assert!(ceiling_half.admits(&answer_task(0.5, 0.1, 0, 0)));
        assert!(!ceiling_half.admits(&answer_task(0.1, 0.6, 0, 0)));
    }

    #[test]
    fn family_mismatch_is_never_admitted() {
        let edit_conv = &ScoringConvention::admissible_edit()[0];
        let answer_conv = &ScoringConvention::admissible_answer()[0];
        assert!(!edit_conv.admits(&answer_task(1.0, 1.0, 0, 0)));
        assert!(!answer_conv.admits(&edit_task(0.5, 1)));
    }

    #[test]
    fn canonical_admits_everything_within_its_family() {
        let canon_edit = ScoringConvention::canonical(ConventionFamily::Edit);
        assert!(canon_edit.admits(&edit_task(0.0, u32::MAX)));
        let canon_answer = ScoringConvention::canonical(ConventionFamily::Answer);
        assert!(canon_answer.admits(&answer_task(0.0, 0.0, u32::MAX, u32::MAX)));
    }

    #[test]
    fn admissible_sets_are_family_uniform_and_distinctly_named() {
        for (set, family) in [
            (ScoringConvention::admissible_edit(), ConventionFamily::Edit),
            (
                ScoringConvention::admissible_answer(),
                ConventionFamily::Answer,
            ),
        ] {
            assert_eq!(set.len(), 4);
            assert!(set.iter().all(|c| c.family == family));
        }
        // The locality-bound convention names never collide across families, so a
        // report reader can tell which family was exercised from the names alone.
        assert!(ScoringConvention::admissible_answer()
            .iter()
            .any(|c| c.name == "trace_locality_floor"));
        assert!(ScoringConvention::admissible_edit()
            .iter()
            .all(|c| !c.name.starts_with("trace_")));
    }
}
