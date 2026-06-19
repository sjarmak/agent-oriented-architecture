use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::common::ConditionedOn;
use crate::input::{Confidence, IndexQuality, MetricInput, SymbolGraph};

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

/// Writable nodes reachable from any node within `k` hops over directed edges.
/// A degraded index yields an empty set: a degraded index never *raises* the
/// mutation surface (R-silent).
fn reachable_writable(graph: &SymbolGraph, k: u32) -> BTreeSet<String> {
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
    let mut queue: VecDeque<(&str, u32)> = graph.nodes.iter().map(|n| (n.as_str(), 0)).collect();

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

/// Compute mutation-surface over the input's symbol graph at depth `<= k`.
pub fn compute_mutation_surface(input: &MetricInput) -> MutationSurface {
    let reachable = reachable_writable(&input.graph, input.k);

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
