use serde::{Deserialize, Serialize};

use crate::common::ConditionedOn;
use crate::edit::{compute_edit_locality, EditLocality};
use crate::error::MetricError;
use crate::input::{Confidence, MetricInputRef};
use crate::invariant::{compute_invariant_discoverability, InvariantDiscoverability};
use crate::mutation::{compute_mutation_surface, MutationSurface};
use crate::retrieval::{compute_retrieval_locality, RetrievalLocality};

/// The combined metric record for a single task run: all four metrics plus the
/// cross-cutting conditioning, confidence, and R0-eligibility flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricRecord {
    pub retrieval_locality: RetrievalLocality,
    pub edit_locality: EditLocality,
    pub invariant_discoverability: InvariantDiscoverability,
    pub mutation_surface: MutationSurface,
    /// All metrics are conditioned on held-out success.
    pub conditioned_on: ConditionedOn,
    /// Whether this run is counted as a success: only held-out passes count, so
    /// a visible pass that fails the held-out check is `false`.
    pub counted_as_success: bool,
    /// Index-quality confidence label (R15).
    pub confidence: Confidence,
    /// Per-record score weight: degraded indexes contribute zero (R-silent).
    pub weight: f64,
    /// Whether the repo may vote in R0: false for a degraded/empty index.
    pub repo_eligible_for_r0: bool,
}

/// Compute the full metric record from a single task-run input.
pub fn compute_metrics(input: MetricInputRef<'_>) -> Result<MetricRecord, MetricError> {
    let quality = input.graph.quality;
    Ok(MetricRecord {
        retrieval_locality: compute_retrieval_locality(input),
        edit_locality: compute_edit_locality(input)?,
        invariant_discoverability: compute_invariant_discoverability(input),
        mutation_surface: compute_mutation_surface(input),
        conditioned_on: ConditionedOn::HeldOut,
        counted_as_success: input.held_out_success,
        confidence: quality.confidence(),
        weight: quality.weight(),
        repo_eligible_for_r0: quality.eligible_for_r0(),
    })
}
