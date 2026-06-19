use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use aoa_trace::SpanType;

use crate::common::{is_read_span, span_artifact, ConditionedOn};
use crate::input::{Confidence, MetricInput};

/// Invariant-discoverability: whether the invariant set `I_t` was accessed via a
/// file.read or symbol.lookup span before the first write.attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvariantDiscoverability {
    /// True iff an anchored invariant artifact was read before the first write.
    pub accessed_before_first_write: bool,
    /// `seq` of the first write.attempt, if any write was attempted.
    pub first_write_seq: Option<u64>,
    /// The anchored invariant names used for matching.
    pub anchored_invariants: BTreeSet<String>,
    pub conditioned_on: ConditionedOn,
    pub confidence: Confidence,
    pub weight: f64,
}

/// Compute invariant-discoverability. When no write was attempted, any invariant
/// read at all counts as discovered-before-write (the write boundary is open).
pub fn compute_invariant_discoverability(input: &MetricInput) -> InvariantDiscoverability {
    let anchored: BTreeSet<String> = input.transform.anchor(&input.invariant_set);

    let mut spans: Vec<_> = input.trace.spans.iter().collect();
    spans.sort_by_key(|s| s.seq);

    let first_write_seq = spans
        .iter()
        .find(|s| s.span_type == SpanType::WriteAttempt)
        .map(|s| s.seq);

    let accessed_before = spans.iter().any(|s| {
        if !is_read_span(s) {
            return false;
        }
        if let Some(boundary) = first_write_seq {
            if s.seq >= boundary {
                return false;
            }
        }
        span_artifact(s).is_some_and(|a| anchored.contains(a))
    });

    InvariantDiscoverability {
        accessed_before_first_write: accessed_before,
        first_write_seq,
        anchored_invariants: anchored,
        conditioned_on: ConditionedOn::HeldOut,
        confidence: input.graph.quality.confidence(),
        weight: input.graph.quality.weight(),
    }
}
