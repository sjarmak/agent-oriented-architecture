use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::common::{is_access_span, ranked_results, span_artifact, ConditionedOn};
use crate::input::{Confidence, MetricInputRef};

/// Retrieval-locality bundle: time-to-first-relevant, Recall@k, and MRR, with
/// the gold set anchored to base-repo symbols through the transform map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalLocality {
    /// 1-based count of tool-call spans up to and including the first access of
    /// an anchored gold artifact. `None` when no gold artifact was ever accessed.
    pub tool_calls_to_first_relevant_artifact: Option<u32>,
    /// Recall@k over the first ranked retrieval batch: anchored gold hits in the
    /// first `k` results divided by gold-set size.
    pub recall_at_k: f64,
    /// Mean reciprocal rank of the first anchored gold artifact in the first
    /// ranked retrieval batch (0.0 when none is present).
    pub mrr: f64,
    /// The `k` used for Recall@k, emitted as data.
    pub k: u32,
    /// The anchored gold names actually used for matching.
    pub anchored_gold: BTreeSet<String>,
    pub conditioned_on: ConditionedOn,
    pub confidence: Confidence,
    pub weight: f64,
}

/// Compute retrieval-locality. `G_t` is anchored to migrated names so a renamed
/// gold symbol still matches the migrated identifier the trace references.
pub fn compute_retrieval_locality(input: MetricInputRef<'_>) -> RetrievalLocality {
    let anchored: BTreeSet<String> = input.transform.anchor(input.gold_set);

    let mut spans: Vec<_> = input.trace.spans.iter().collect();
    spans.sort_by_key(|s| s.seq);

    let mut tool_calls_to_first = None;
    let mut count = 0u32;
    for span in &spans {
        if !is_access_span(span) {
            continue;
        }
        count += 1;
        if let Some(artifact) = span_artifact(span) {
            if anchored.contains(artifact) {
                tool_calls_to_first = Some(count);
                break;
            }
        }
    }

    let first_batch = spans
        .iter()
        .find(|s| !ranked_results(s).is_empty())
        .map(|s| ranked_results(s))
        .unwrap_or_default();

    let k = input.k;
    let top_k = first_batch.iter().take(k as usize).copied();
    let hits_in_k = top_k.filter(|r| anchored.contains(*r)).count();
    let recall_at_k = if anchored.is_empty() {
        0.0
    } else {
        hits_in_k as f64 / anchored.len() as f64
    };

    let mrr = first_batch
        .iter()
        .position(|r| anchored.contains(*r))
        .map(|pos| 1.0 / (pos as f64 + 1.0))
        .unwrap_or(0.0);

    RetrievalLocality {
        tool_calls_to_first_relevant_artifact: tool_calls_to_first,
        recall_at_k,
        mrr,
        k,
        anchored_gold: anchored,
        conditioned_on: ConditionedOn::HeldOut,
        confidence: input.graph.quality.confidence(),
        weight: input.graph.quality.weight(),
    }
}
