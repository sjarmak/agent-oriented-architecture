use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::common::ConditionedOn;
use crate::input::{Confidence, IndexQuality, MetricInputRef, SymbolGraph};

/// Mutation-surface: the count of writable files reachable in the symbol graph
/// at depth `<= k`. This is an over-approximation of the true blast radius.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationSurface {
    /// Number of distinct writable nodes reachable within depth `k`.
    pub writable_reachable: usize,
    /// The reachable writable node identifiers (deterministic order).
    pub reachable: BTreeSet<String>,
    /// The reachability depth bound, emitted as data.
    pub k: u32,
    /// Always true: this metric over-approximates the mutation blast radius.
    pub over_approximation: bool,
    pub conditioned_on: ConditionedOn,
    pub confidence: Confidence,
    pub weight: f64,
}

/// Writable nodes reachable from the seed nodes within `k` hops over directed
/// edges. A degraded index yields an empty set: a degraded index never
/// *raises* the mutation surface (R-silent). Seeds themselves count when
/// writable (depth 0).
pub(crate) fn reachable_writable_from<'g>(
    graph: &'g SymbolGraph,
    seeds: impl IntoIterator<Item = &'g str>,
    k: u32,
) -> BTreeSet<String> {
    if graph.quality == IndexQuality::Degraded {
        return BTreeSet::new();
    }

    let adjacency: Vec<(&str, &str)> = graph
        .edges
        .iter()
        .map(|(f, t)| (f.as_str(), t.as_str()))
        .collect();

    let mut reachable = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut queue: VecDeque<(&str, u32)> = seeds.into_iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = queue.pop_front() {
        if !visited.insert(node.to_string()) {
            continue;
        }
        if graph.writable.contains(node) {
            reachable.insert(node.to_string());
        }
        if depth >= k {
            continue;
        }
        for (from, to) in &adjacency {
            if *from == node {
                queue.push_back((to, depth + 1));
            }
        }
    }

    reachable
}

/// Compute mutation-surface over the input's symbol graph at depth `<= k`,
/// seeded from every node.
pub fn compute_mutation_surface(input: MetricInputRef<'_>) -> MutationSurface {
    let seeds = input.graph.nodes.iter().map(String::as_str);
    let reachable = reachable_writable_from(input.graph, seeds, input.k);

    MutationSurface {
        writable_reachable: reachable.len(),
        reachable,
        k: input.k,
        over_approximation: true,
        conditioned_on: ConditionedOn::HeldOut,
        confidence: input.graph.quality.confidence(),
        weight: input.graph.quality.weight(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn chain_graph(quality: IndexQuality) -> SymbolGraph {
        SymbolGraph {
            nodes: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            edges: vec![
                ("a".to_string(), "b".to_string()),
                ("b".to_string(), "c".to_string()),
            ],
            writable: ["b", "c"].iter().map(|s| s.to_string()).collect(),
            node_paths: BTreeMap::new(),
            quality,
        }
    }

    #[test]
    fn seeded_reachability_is_bounded_by_k_from_the_seeds_only() {
        let graph = chain_graph(IndexQuality::Scip);
        // From `a` with k=1: b is reachable, c is one hop too far.
        let reached = reachable_writable_from(&graph, ["a"], 1);
        assert_eq!(reached, ["b".to_string()].into());
        // From `b` with k=1: b itself (depth 0, writable) and c.
        let reached = reachable_writable_from(&graph, ["b"], 1);
        assert_eq!(reached, ["b".to_string(), "c".to_string()].into());
    }

    #[test]
    fn degraded_index_never_raises_the_seeded_surface() {
        let graph = chain_graph(IndexQuality::Degraded);
        assert!(reachable_writable_from(&graph, ["a"], 5).is_empty());
    }
}
