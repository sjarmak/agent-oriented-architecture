use serde::{Deserialize, Serialize};

use crate::types::PairTask;

/// Which edit-locality bound a convention applies when admitting a task.
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
pub struct ScoringConvention {
    /// Human-readable label, emitted in the report.
    pub name: String,
    /// Which edit-locality bound to apply.
    pub locality_bound: LocalityBound,
    /// The locality threshold the bound is measured against.
    pub locality_threshold: f64,
    /// Maximum admissible mutation-surface depth; tasks deeper than this are
    /// excluded under this convention.
    pub max_mutation_depth: u32,
    /// Weight applied to a repo-arm success when counting held-out success.
    pub repo_weight: f64,
    /// Weight applied to a harness-arm success when counting held-out success.
    pub harness_weight: f64,
}

impl ScoringConvention {
    /// Whether a task is admitted for scoring under this convention.
    pub fn admits(&self, task: &PairTask) -> bool {
        if task.mutation_depth > self.max_mutation_depth {
            return false;
        }
        match self.locality_bound {
            LocalityBound::Floor => task.edit_locality >= self.locality_threshold,
            LocalityBound::Ceiling => task.edit_locality <= self.locality_threshold,
        }
    }

    /// The default admissible set: edit-locality floor and ceiling, a deeper
    /// mutation-surface depth-k, and an alternative metric-weighting. All of
    /// these are extremes a real `proceed` must survive.
    pub fn admissible_default() -> Vec<Self> {
        vec![
            Self {
                name: "edit_locality_floor".to_string(),
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_mutation_depth: u32::MAX,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "edit_locality_ceiling".to_string(),
                locality_bound: LocalityBound::Ceiling,
                locality_threshold: 1.0,
                max_mutation_depth: u32::MAX,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "mutation_surface_depth_k".to_string(),
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_mutation_depth: 3,
                repo_weight: 1.0,
                harness_weight: 1.0,
            },
            Self {
                name: "alternative_metric_weights".to_string(),
                locality_bound: LocalityBound::Floor,
                locality_threshold: 0.0,
                max_mutation_depth: u32::MAX,
                repo_weight: 0.75,
                harness_weight: 1.25,
            },
        ]
    }

    /// The canonical convention used for the report's emitted deltas: no task
    /// exclusion and equal weights.
    pub fn canonical() -> Self {
        Self {
            name: "canonical".to_string(),
            locality_bound: LocalityBound::Floor,
            locality_threshold: 0.0,
            max_mutation_depth: u32::MAX,
            repo_weight: 1.0,
            harness_weight: 1.0,
        }
    }
}
