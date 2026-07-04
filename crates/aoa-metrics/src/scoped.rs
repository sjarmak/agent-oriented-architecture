//! Per-subtree metric aggregation: the same metric extractors, run on a
//! subtree-filtered view of the task input.
//!
//! Spans are attributed to subtrees by their `path` attribute (the target file
//! path); spans that name no path, or a path outside every member, are
//! unattributable and appear in no subtree row. Gold symbols are attributed by
//! their anchored (migrated) name; graph nodes by their recorded `node_paths`
//! entry. Rows are emitted only for subtrees with attributed activity (at
//! least one span or edited file), in lexicographic order; repo-wide numbers
//! are computed elsewhere and are never altered here.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use aoa_trace::{Span, Trace};

use crate::edit::{compute_edit_locality, EditLocality};
use crate::error::MetricError;
use crate::input::MetricInputRef;
use crate::mutation::reachable_writable_from;
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
    /// Writable nodes inside this subtree reachable within `k` hops from this
    /// subtree's own nodes (per-subtree mutation surface, aoa-d6t.30). `None`
    /// when no graph node attributes to this subtree — see
    /// `mutation_unavailable`. Never fabricated: a subtree with zero seed
    /// nodes has an *unknown* blast radius, not a zero one.
    pub mutation_surface: Option<usize>,
    /// Writable nodes attributed to a *different* subtree reachable within `k`
    /// hops from this subtree's nodes (cross-subtree mutation leakage). Nodes
    /// with no recorded path attribute to no subtree and count in neither
    /// number, mirroring span attribution. `None` exactly when
    /// `mutation_surface` is.
    pub mutation_leakage: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation_unavailable: Option<String>,
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

    // Gold is attributed by its anchored (migrated) name — the name the trace
    // actually references — but carried as base names so the extractor's own
    // anchoring still applies. Grouped in one pass like the maps above.
    let mut gold_by: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for base in input.gold_set {
        let anchored = input
            .transform
            .base_to_migrated
            .get(base)
            .map(String::as_str)
            .unwrap_or(base);
        if let Some(subtree) = partition.attribute(anchored) {
            gold_by.entry(subtree).or_default().insert(base.clone());
        }
    }

    // Per-subtree views must keep one entry per accepted solution (possibly
    // empty) so the ≥2-solutions edit-locality precondition is judged on the
    // solution count, not on how many touched this subtree.
    let solution_count = input.accepted_solutions.len();
    let mut solutions_by: BTreeMap<&str, Vec<BTreeSet<String>>> = BTreeMap::new();
    for (i, solution) in input.accepted_solutions.iter().enumerate() {
        for file in solution {
            if let Some(subtree) = partition.attribute(file) {
                solutions_by
                    .entry(subtree)
                    .or_insert_with(|| vec![BTreeSet::new(); solution_count])[i]
                    .insert(file.clone());
            }
        }
    }

    // Graph nodes attributed to subtrees by their recorded file path; nodes
    // with no path (or a path outside every member) attribute to nothing.
    let node_subtrees: BTreeMap<&str, &str> = input
        .graph
        .node_paths
        .iter()
        .filter_map(|(node, path)| {
            partition
                .attribute(path)
                .map(|subtree| (node.as_str(), subtree))
        })
        .collect();

    // Active subtrees only; BTreeSet gives the lexicographic row order.
    let active: BTreeSet<&str> = spans_by.keys().chain(edited_by.keys()).copied().collect();

    active
        .into_iter()
        .map(|subtree| {
            let trace = Trace {
                spans: spans_by.remove(subtree).unwrap_or_default(),
            };
            let gold_set = gold_by.remove(subtree).unwrap_or_default();
            let edited_files = edited_by.remove(subtree).unwrap_or_default();
            let accepted_solutions = solutions_by
                .remove(subtree)
                .unwrap_or_else(|| vec![BTreeSet::new(); solution_count]);

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

            // Per-subtree mutation numbers (aoa-d6t.30): k-bounded BFS seeded
            // from this subtree's own nodes; reached writable nodes are split
            // into in-subtree surface vs cross-subtree leakage. Zero seeds —
            // an empty or path-less index (legacy vendored SCIP JSON carries
            // no document relative_path), or a subtree with no indexed
            // symbols — means the split is unknowable, and a 0/0 row would
            // read as "isolated" at the graph's confidence. Marked
            // unavailable instead, mirroring `edit_locality_unavailable`.
            let seeds: Vec<&str> = node_subtrees
                .iter()
                .filter(|(_, s)| **s == subtree)
                .map(|(node, _)| *node)
                .collect();
            let (mutation_surface, mutation_leakage, mutation_unavailable) = if seeds.is_empty() {
                (
                    None,
                    None,
                    Some("no graph nodes attributed to this subtree".to_string()),
                )
            } else {
                let reached = reachable_writable_from(input.graph, seeds, input.k);
                let (inside, outside) = reached.iter().fold((0, 0), |(inside, outside), node| {
                    match node_subtrees.get(node.as_str()) {
                        Some(s) if *s == subtree => (inside + 1, outside),
                        Some(_) => (inside, outside + 1),
                        // No recorded path: attributable to no subtree.
                        None => (inside, outside),
                    }
                });
                (Some(inside), Some(outside), None)
            };

            SubtreeMetrics {
                subtree: subtree.to_string(),
                attributed_span_count: trace.spans.len(),
                edited_file_count: edited_files.len(),
                retrieval_locality: compute_retrieval_locality(view),
                edit_locality,
                edit_locality_unavailable,
                mutation_surface,
                mutation_leakage,
                mutation_unavailable,
            }
        })
        .collect()
}
