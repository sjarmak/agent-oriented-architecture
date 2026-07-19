use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::common::{is_read_span, span_artifact, ConditionedOn};
use crate::input::{Confidence, MetricInputRef};

/// Invariant-discoverability: whether the invariant set `I_t` was accessed via a
/// file.read or symbol.lookup span before the agent began writing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvariantDiscoverability {
    /// True iff an anchored invariant artifact was read before the first write.
    pub accessed_before_first_write: bool,
    /// `seq` of the span that opened the write boundary, if the agent wrote.
    pub first_write_seq: Option<u64>,
    /// The anchored invariant names used for matching.
    pub anchored_invariants: BTreeSet<String>,
    pub conditioned_on: ConditionedOn,
    pub confidence: Confidence,
    pub weight: f64,
}

/// Compute invariant-discoverability. When the agent never wrote, any invariant
/// read at all counts as discovered-before-write (the write boundary is open).
///
/// The boundary comes from [`aoa_trace::SpanType::opens_write_boundary`] rather than from
/// `write.attempt` alone. Matching the attempt was correct only on the hook
/// path: the codeprobe shim settles a correlated write by rewriting the attempt
/// *in place*, so a reconstructed transcript whose writes all landed retains no
/// `write.attempt` at all. That left the boundary open on exactly the traces
/// that did the most writing, and every post-edit read was then counted as
/// discovered-before-write — a well-formed, silently wrong measurement.
pub fn compute_invariant_discoverability(input: MetricInputRef<'_>) -> InvariantDiscoverability {
    let anchored: BTreeSet<String> = input.transform.anchor(input.invariant_set);

    let mut spans: Vec<_> = input.trace.spans.iter().collect();
    spans.sort_by_key(|s| s.seq);

    let first_write_seq = spans
        .iter()
        .find(|s| s.span_type.opens_write_boundary())
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
