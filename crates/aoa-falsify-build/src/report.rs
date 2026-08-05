//! The build report emitted alongside the `FalsifyInput` and consumed by
//! `aoa falsify --build-meta`.

use serde::Serialize;

use aoa_gap::{ExposureStatus, HeldOutProvenance};
use aoa_metrics::Confidence;

use crate::manifest::TaskShape;

/// One task dropped from a repo's identical-pair set, with the reason.
#[derive(Debug, Serialize)]
pub struct ExcludedTask {
    pub task_id: String,
    pub reason: String,
}

/// Per-repo build provenance: what was assembled and why.
#[derive(Debug, Serialize)]
pub struct RepoBuild {
    pub repo_id: String,
    pub identical_pairs: usize,
    pub candidate_pairs: usize,
    pub pair_yield: f64,
    pub holdout_size: u32,
    pub native_span: HeldOutProvenance,
    pub confidence: Confidence,
    pub calibrated: bool,
    pub exposure: ExposureStatus,
    /// Whether this repo satisfies the gate's eligibility predicate (high +
    /// native-composed + calibrated + unexposed). Informational — the gate
    /// re-derives it.
    pub eligible: bool,
    pub excluded_tasks: Vec<ExcludedTask>,
}

/// A repo dropped from the input (no identical pairs in some run), with the
/// per-task exclusion reasons that explain the drop. Eligibility facts are
/// deliberately absent: they are meaningless for a repo that supplies no
/// evidence.
#[derive(Debug, Serialize)]
pub struct DroppedRepo {
    pub repo_id: String,
    pub candidate_pairs: usize,
    pub pair_yield: f64,
    pub excluded_tasks: Vec<ExcludedTask>,
}

/// The build report. `convention_inputs_degraded` is the load-bearing flag
/// `aoa falsify --build-meta` reads to decide whether to abstain.
///
/// [`build`](crate::build) returns the report with an empty `out_path`: only
/// the caller knows where it wrote the input. Stamp it with
/// [`BuildReport::with_artifacts`] before serializing rather than leaving
/// mutable holes in a serialized artifact.
#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub out_path: String,
    pub observations_path: String,
    pub observations_sha256: String,
    pub observation_count: usize,
    pub observation_ids: Vec<String>,
    pub repo_count: usize,
    pub total_identical_pairs: usize,
    /// The (uniform) task shape of the manifest's repos, as data.
    pub task_shape: TaskShape,
    pub convention_inputs_degraded: bool,
    pub repos: Vec<RepoBuild>,
    /// Repos that contributed no identical pairs and were dropped from the
    /// input — kept here so their per-task exclusion reasons stay inspectable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped_repos: Vec<DroppedRepo>,
    pub notes: Vec<String>,
}

impl BuildReport {
    /// Record the input and observation-sidecar identities. Consuming, so a
    /// report cannot be serialized half-stamped by accident.
    #[must_use]
    pub fn with_artifacts(
        mut self,
        out_path: String,
        observations_path: String,
        observations_sha256: String,
    ) -> Self {
        self.out_path = out_path;
        self.observations_path = observations_path;
        self.observations_sha256 = observations_sha256;
        self
    }
}
