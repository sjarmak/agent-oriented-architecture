//! Per-subtree metric aggregation: the same metric extractors, run on a
//! subtree-filtered view of the task input.
//!
//! Spans are attributed to subtrees by their `path` attribute (the target file
//! path); spans that name no path, or a path outside every member, are
//! unattributable and appear in no subtree row. Gold symbols are attributed by
//! their anchored (migrated) name. Rows are emitted only for subtrees with
//! attributed activity (at least one span or edited file), in lexicographic
//! order; repo-wide numbers are computed elsewhere and are never altered here.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use aoa_trace::{Span, Trace};

use crate::edit::{compute_edit_locality, EditLocality};
use crate::error::MetricError;
use crate::input::MetricInputRef;
use crate::retrieval::{compute_retrieval_locality, RetrievalLocality};
use crate::subtree::SubtreePartition;

/// One subtree's metric row for a single task run.
///
/// Per-subtree retrieval and edit locality reuse the repo-wide extractors on
/// the filtered view. When the task's gold set has no member attributable to
/// this subtree, `retrieval_locality.anchored_gold` is empty and its recall/MRR
/// read 0.0 — the empty anchor set on the row makes that visible, it is not a
/// judgment that retrieval failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtreeMetrics {
    /// The member dir this row aggregates (relative to the repo root).
    pub subtree: String,
    /// Spans whose `path` attribute attributed to this subtree.
    pub attributed_span_count: usize,
    /// Edited files (`F_edit`) attributed to this subtree.
    pub edited_file_count: usize,
    pub retrieval_locality: RetrievalLocality,
    /// `None` when fewer than two accepted solutions exist — see
    /// `edit_locality_unavailable`. Never fabricated.
    pub edit_locality: Option<EditLocality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_locality_unavailable: Option<String>,
}

/// Compute per-subtree metric rows by filtering the input per member and
/// re-running the existing extractors on each filtered view.
pub fn compute_subtree_metrics(
    input: MetricInputRef<'_>,
    partition: &SubtreePartition,
) -> Vec<SubtreeMetrics> {
    let mut spans_by: BTreeMap<&str, Vec<Span>> = BTreeMap::new();
    for span in &input.trace.spans {
        let Some(path) = span.attributes.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(subtree) = partition.attribute(path) else {
            continue;
        };
        spans_by.entry(subtree).or_default().push(span.clone());
    }

    let mut edited_by: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for file in input.edited_files {
        if let Some(subtree) = partition.attribute(file) {
            edited_by.entry(subtree).or_default().insert(file.clone());
        }
    }

    // Active subtrees only; BTreeSet gives the lexicographic row order.
    let active: BTreeSet<&str> = spans_by.keys().chain(edited_by.keys()).copied().collect();

    active
        .into_iter()
        .map(|subtree| {
            let trace = Trace {
                spans: spans_by.remove(subtree).unwrap_or_default(),
            };
            // Gold is attributed by its anchored (migrated) name — the name the
            // trace actually references — but passed through as base names so
            // the extractor's own anchoring still applies.
            let gold_set: BTreeSet<String> = input
                .gold_set
                .iter()
                .filter(|base| {
                    let anchored = input
                        .transform
                        .base_to_migrated
                        .get(*base)
                        .map(String::as_str)
                        .unwrap_or(base);
                    partition.attribute(anchored) == Some(subtree)
                })
                .cloned()
                .collect();
            let edited_files = edited_by.remove(subtree).unwrap_or_default();
            let accepted_solutions: Vec<BTreeSet<String>> = input
                .accepted_solutions
                .iter()
                .map(|solution| {
                    solution
                        .iter()
                        .filter(|f| partition.attribute(f) == Some(subtree))
                        .cloned()
                        .collect()
                })
                .collect();

            let view = MetricInputRef {
                trace: &trace,
                gold_set: &gold_set,
                edited_files: &edited_files,
                accepted_solutions: &accepted_solutions,
                ..input
            };

            // Exhaustive on MetricError's sole variant, mirroring the CLI: a
            // future variant must be handled deliberately, never silently nulled.
            let (edit_locality, edit_locality_unavailable) = match compute_edit_locality(view) {
                Ok(e) => (Some(e), None),
                Err(MetricError::InsufficientAcceptedSolutions(n)) => (
                    None,
                    Some(format!("insufficient accepted solutions: {n} (need ≥2)")),
                ),
            };

            SubtreeMetrics {
                subtree: subtree.to_string(),
                attributed_span_count: trace.spans.len(),
                edited_file_count: edited_files.len(),
                retrieval_locality: compute_retrieval_locality(view),
                edit_locality,
                edit_locality_unavailable,
            }
        })
        .collect()
}
