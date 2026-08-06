use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::common::{is_access_span, ranked_results, span_artifact, ConditionedOn};
use crate::error::MetricError;
use crate::input::{Confidence, MetricInputRef};

/// Retrieval-locality bundle: time-to-first-relevant, Recall@k, and MRR, with
/// the gold set anchored to base-repo symbols through the transform map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalLocality {
    /// 1-based count of tool-call spans up to and including the first access of
    /// an anchored gold artifact. `None` when no gold artifact was ever accessed.
    pub tool_calls_to_first_relevant_artifact: Option<u32>,
    /// Recall@k over the first ranked retrieval batch: anchored gold hits in the
    /// first `k` results divided by gold-set size. `None` when that denominator
    /// is empty, or when the trace carries no ranked batch to measure.
    pub recall_at_k: Option<f64>,
    /// Mean reciprocal rank of the first anchored gold artifact in the first
    /// ranked retrieval batch. `Some(0.0)` means a retriever ranked results and
    /// none of them were gold; `None` means there was no anchored gold set, or
    /// no ranked batch at all, to measure against.
    pub mrr: Option<f64>,
    /// Why Recall@k and MRR are unavailable. Present exactly when one of the two
    /// inputs is missing: an empty anchored gold set (a 0/0 recall row is not a
    /// retrieval observation) or a trace with no ranked-results span (absent
    /// evidence is not a measured zero).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
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
///
/// Fails when a span carries a malformed `results` attribute; a broken
/// measurement artifact is an error, never a silently empty batch.
pub fn compute_retrieval_locality(
    input: MetricInputRef<'_>,
) -> Result<RetrievalLocality, MetricError> {
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

    // Select on PRESENCE of the `results` attribute, not on a non-empty list. A
    // retriever that ran and ranked nothing is a real observation — recall 0.0
    // is the truthful measure — so an empty batch must reach the scoring arm
    // below rather than be skipped and reported as absent evidence. Only a
    // wholly absent `results` key across every span means no evidence.
    let mut first_batch = None;
    for span in &spans {
        if let Some(results) = ranked_results(span)? {
            first_batch = Some(results);
            break;
        }
    }

    let k = input.k;
    let (recall_at_k, mrr, unavailable) = match (anchored.is_empty(), first_batch) {
        (true, _) => (
            None,
            None,
            Some("no gold symbols anchored through the transform map".to_string()),
        ),
        (false, None) => (
            None,
            None,
            Some("no ranked-results span in trace".to_string()),
        ),
        (false, Some(batch)) => {
            let hits_in_k = batch
                .iter()
                .take(k as usize)
                .copied()
                .filter(|result| anchored.contains(*result))
                .count();
            let recall_at_k = hits_in_k as f64 / anchored.len() as f64;
            let mrr = batch
                .iter()
                .position(|r| anchored.contains(*r))
                .map(|pos| 1.0 / (pos as f64 + 1.0))
                .unwrap_or(0.0);
            (Some(recall_at_k), Some(mrr), None)
        }
    };

    Ok(RetrievalLocality {
        tool_calls_to_first_relevant_artifact: tool_calls_to_first,
        recall_at_k,
        mrr,
        unavailable,
        k,
        anchored_gold: anchored,
        conditioned_on: ConditionedOn::HeldOut,
        confidence: input.graph.quality.confidence(),
        weight: input.graph.quality.weight(),
    })
}
