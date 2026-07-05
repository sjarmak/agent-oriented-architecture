//! Answer-task convention inputs: trace-locality and trace-reach depth.
//!
//! Answer-shaped comprehension tasks have no accepted PR solutions, so the
//! edit-task convention inputs (edit-locality, mutation depth) are undefined
//! for them. Their analogues — pre-registered 2026-07-04 (bead aoa-dhk.1; see
//! `docs/r0_runbook.md` § "Answer-task convention set") — derive from what a
//! comprehension trial actually produces:
//!
//! - **trace-locality** `= |T ∩ O| / |T|`, where `T` is the trace footprint
//!   (the repo files the agent's instrumented trace read or touched) and `O`
//!   is the task's oracle chain files. `1.0` means every touched file was on
//!   the oracle chain (focused navigation); values near `0.0` mean repo-wide
//!   thrashing.
//! - **trace-reach depth** `= max over oracle files of the minimum undirected
//!   symbol-graph distance from the trace footprint`, i.e. the smallest `k`
//!   such that every oracle chain file is within `k` hops of some node defined
//!   in a touched file. Reference edges are traversed in both directions:
//!   reach measures navigational proximity, not mutation blast radius.
//!
//! Both are mechanical joins over artifacts that exist per trial (transcript
//! trace, mined oracle, vendored SCIP index); nothing is fabricated. When an
//! input cannot be computed the reason is surfaced as a typed error — never a
//! silently admitting default.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use aoa_trace::{SpanType, Trace};

use crate::input::{IndexQuality, SymbolGraph};

/// Trace-reach outcome: the depth, or an explicit "the oracle chain is not
/// reachable from the footprint at any depth". Callers decide the encoding of
/// unreachability (the falsification gate saturates it to `u32::MAX`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceReach {
    Depth(u32),
    Unreachable,
}

/// Why answer-task convention inputs could not be computed for a trial.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TraceInputError {
    /// The trace touched no file that resolves into the repo's file universe:
    /// there is no footprint to measure locality or reach from.
    #[error("trace footprint is empty: no instrumented repo-file access in the trace")]
    EmptyFootprint,
    /// The task supplied no oracle chain files (unresolvable against the repo
    /// universe), so there is nothing to measure the footprint against.
    #[error("oracle chain is empty: no oracle files resolve against the repo universe")]
    EmptyOracleChain,
    /// A degraded index carries no graph; reach cannot be measured (R-silent:
    /// a degraded index never fabricates evidence).
    #[error("symbol graph is degraded: trace reach is not measurable")]
    DegradedIndex,
    /// No graph node is defined in any footprint file — the index cannot seed
    /// the reach search. (Unreachable via the builder, which derives both the
    /// footprint and the oracle chain from the index's own file universe.)
    #[error("no symbol-graph node is defined in any footprint file")]
    NoSeedNodes,
    /// An oracle chain file defines no graph node, so its distance is
    /// undefined. (Unreachable via the builder, as above.)
    #[error("oracle chain file {0:?} defines no symbol-graph node")]
    OracleFileNotIndexed(String),
}

/// The two answer-task convention inputs for one arm's trial.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceConventionInputs {
    /// `|T ∩ O| / |T|`, in `[0.0, 1.0]`.
    pub trace_locality: f64,
    pub trace_reach: TraceReach,
}

/// Resolve a raw trace path onto the repo-relative file universe.
///
/// Trace paths may be absolute (worktree checkouts) or repo-relative; universe
/// paths are repo-relative (SCIP document `relative_path`). A raw path matches
/// a universe path when it equals it or ends with `/<universe path>` (component
/// aligned). The longest — most specific — universe match wins, so a repo-root
/// file can never shadow a nested one. Returns `None` for paths outside the
/// universe (workspace scratch files, `answer.json`, out-of-repo reads).
pub fn match_repo_relative(raw: &str, universe: &BTreeSet<String>) -> Option<String> {
    universe
        .iter()
        .filter(|u| raw == u.as_str() || raw.ends_with(&format!("/{u}")))
        .max_by_key(|u| u.len())
        .cloned()
}

/// The trace footprint `T`: the distinct repo files the agent's instrumented
/// trace read or touched (`file.read`, `write.attempt`, `write.blocked` span
/// `path` targets), resolved onto the repo universe. Paths that resolve to no
/// universe file are dropped — they are workspace/protocol artifacts, not repo
/// navigation.
pub fn trace_footprint(trace: &Trace, universe: &BTreeSet<String>) -> BTreeSet<String> {
    trace
        .spans
        .iter()
        .filter(|s| {
            matches!(
                s.span_type,
                SpanType::FileRead | SpanType::WriteAttempt | SpanType::WriteBlocked
            )
        })
        .filter_map(|s| s.attributes.get("path").and_then(|v| v.as_str()))
        .filter_map(|p| match_repo_relative(p, universe))
        .collect()
}

/// Compute both answer-task convention inputs for one trial.
///
/// `footprint` and `oracle_chain` must already be repo-relative (resolved via
/// [`match_repo_relative`] / the oracle-chain resolver), which the builder
/// guarantees by deriving both from the same index universe.
pub fn compute_trace_convention_inputs(
    graph: &SymbolGraph,
    footprint: &BTreeSet<String>,
    oracle_chain: &BTreeSet<String>,
) -> Result<TraceConventionInputs, TraceInputError> {
    if footprint.is_empty() {
        return Err(TraceInputError::EmptyFootprint);
    }
    if oracle_chain.is_empty() {
        return Err(TraceInputError::EmptyOracleChain);
    }

    let on_chain = footprint.intersection(oracle_chain).count();
    let trace_locality = on_chain as f64 / footprint.len() as f64;

    let trace_reach = trace_reach(graph, footprint, oracle_chain)?;

    Ok(TraceConventionInputs {
        trace_locality,
        trace_reach,
    })
}

/// Multi-source BFS over the symbol graph, treated as undirected, from the
/// nodes defined in footprint files to the nodes defined in each oracle file.
fn trace_reach(
    graph: &SymbolGraph,
    footprint: &BTreeSet<String>,
    oracle_chain: &BTreeSet<String>,
) -> Result<TraceReach, TraceInputError> {
    if graph.quality == IndexQuality::Degraded {
        return Err(TraceInputError::DegradedIndex);
    }

    // Undirected adjacency: a reference edge connects navigationally adjacent
    // code in either direction.
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in &graph.edges {
        adjacency.entry(from.as_str()).or_default().push(to);
        adjacency.entry(to.as_str()).or_default().push(from);
    }

    let seeds: Vec<&str> = graph
        .node_paths
        .iter()
        .filter(|(_, path)| footprint.contains(*path))
        .map(|(node, _)| node.as_str())
        .collect();
    if seeds.is_empty() {
        return Err(TraceInputError::NoSeedNodes);
    }

    // node -> its oracle file, for files that still need a distance.
    let mut node_oracle_file: BTreeMap<&str, &str> = BTreeMap::new();
    for (node, path) in &graph.node_paths {
        if oracle_chain.contains(path) {
            node_oracle_file.insert(node.as_str(), path.as_str());
        }
    }
    for file in oracle_chain {
        if !node_oracle_file.values().any(|f| *f == file.as_str()) {
            return Err(TraceInputError::OracleFileNotIndexed(file.clone()));
        }
    }

    let mut pending: BTreeSet<&str> = oracle_chain.iter().map(String::as_str).collect();
    let mut deepest = 0u32;
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<(&str, u32)> = seeds.into_iter().map(|n| (n, 0)).collect();

    while let Some((node, depth)) = queue.pop_front() {
        if !visited.insert(node) {
            continue;
        }
        if let Some(file) = node_oracle_file.get(node) {
            if pending.remove(file) {
                deepest = deepest.max(depth);
                if pending.is_empty() {
                    return Ok(TraceReach::Depth(deepest));
                }
            }
        }
        for next in adjacency.get(node).into_iter().flatten() {
            if !visited.contains(next) {
                queue.push_back((next, depth + 1));
            }
        }
    }

    Ok(TraceReach::Unreachable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_trace::{Span, SpanSource};
    use serde_json::{Map, Value};

    fn universe(files: &[&str]) -> BTreeSet<String> {
        files.iter().map(|s| s.to_string()).collect()
    }

    fn span(span_type: SpanType, path: &str) -> Span {
        let mut attributes = Map::new();
        attributes.insert("path".to_string(), Value::String(path.to_string()));
        Span {
            span_type,
            source: SpanSource::Native,
            seq: 0,
            attributes,
        }
    }

    /// A graph over three files: app.py defines `app` referencing `core`
    /// (defined in core.py), core references `util` (defined in util.py).
    fn chain_graph() -> SymbolGraph {
        SymbolGraph {
            nodes: vec!["app".into(), "core".into(), "util".into()],
            edges: vec![
                ("app".to_string(), "core".to_string()),
                ("core".to_string(), "util".to_string()),
            ],
            writable: BTreeSet::new(),
            node_paths: [
                ("app".to_string(), "src/app.py".to_string()),
                ("core".to_string(), "src/core.py".to_string()),
                ("util".to_string(), "src/util.py".to_string()),
            ]
            .into(),
            quality: IndexQuality::Scip,
        }
    }

    #[test]
    fn match_repo_relative_prefers_the_longest_component_aligned_suffix() {
        let u = universe(&["base.py", "tests/base.py"]);
        // Absolute worktree path resolves to the most specific universe entry.
        assert_eq!(
            match_repo_relative("/work/slot-1/tests/base.py", &u).as_deref(),
            Some("tests/base.py")
        );
        // Already-relative paths match exactly.
        assert_eq!(
            match_repo_relative("base.py", &u).as_deref(),
            Some("base.py")
        );
        // A non-component-aligned suffix (no '/') does not match.
        assert_eq!(match_repo_relative("notbase.py", &u), None);
        // Out-of-repo paths resolve to nothing.
        assert_eq!(match_repo_relative("/tmp/answer.json", &u), None);
    }

    #[test]
    fn trace_footprint_keeps_reads_and_writes_drops_non_repo_paths() {
        let u = universe(&["src/app.py", "src/core.py"]);
        let trace = Trace {
            spans: vec![
                span(SpanType::FileRead, "/w/src/app.py"),
                span(SpanType::WriteAttempt, "/w/answer.json"), // out of repo
                span(SpanType::WriteBlocked, "src/core.py"),
                span(SpanType::RetrievalSearch, "src/app.py"), // query, not a file touch
            ],
        };
        assert_eq!(
            trace_footprint(&trace, &u),
            universe(&["src/app.py", "src/core.py"])
        );
    }

    #[test]
    fn locality_is_on_chain_fraction_of_the_footprint() {
        let graph = chain_graph();
        let footprint = universe(&["src/app.py", "src/util.py"]);
        let oracle = universe(&["src/app.py"]);
        let inputs = compute_trace_convention_inputs(&graph, &footprint, &oracle).unwrap();
        // One of two touched files is on the oracle chain.
        assert_eq!(inputs.trace_locality, 0.5);
        assert_eq!(inputs.trace_reach, TraceReach::Depth(0));
    }

    #[test]
    fn reach_depth_covers_every_oracle_file_and_is_undirected() {
        let graph = chain_graph();
        // The agent only touched util.py; the oracle chain is app.py + core.py.
        // Undirected distances from util: core = 1, app = 2 => depth 2.
        let footprint = universe(&["src/util.py"]);
        let oracle = universe(&["src/app.py", "src/core.py"]);
        let inputs = compute_trace_convention_inputs(&graph, &footprint, &oracle).unwrap();
        assert_eq!(inputs.trace_locality, 0.0);
        assert_eq!(inputs.trace_reach, TraceReach::Depth(2));
    }

    #[test]
    fn disconnected_oracle_is_unreachable_not_a_depth() {
        let mut graph = chain_graph();
        graph.nodes.push("island".into());
        graph
            .node_paths
            .insert("island".into(), "src/island.py".into());
        let footprint = universe(&["src/app.py"]);
        let oracle = universe(&["src/island.py"]);
        let inputs = compute_trace_convention_inputs(&graph, &footprint, &oracle).unwrap();
        assert_eq!(inputs.trace_reach, TraceReach::Unreachable);
    }

    #[test]
    fn unavailable_inputs_surface_typed_reasons() {
        let graph = chain_graph();
        let some = universe(&["src/app.py"]);
        assert_eq!(
            compute_trace_convention_inputs(&graph, &BTreeSet::new(), &some).unwrap_err(),
            TraceInputError::EmptyFootprint
        );
        assert_eq!(
            compute_trace_convention_inputs(&graph, &some, &BTreeSet::new()).unwrap_err(),
            TraceInputError::EmptyOracleChain
        );
        // An oracle file with no defined nodes has no measurable distance.
        let unindexed = universe(&["docs/README.md"]);
        assert_eq!(
            compute_trace_convention_inputs(&graph, &some, &unindexed).unwrap_err(),
            TraceInputError::OracleFileNotIndexed("docs/README.md".to_string())
        );

        let mut degraded = chain_graph();
        degraded.quality = IndexQuality::Degraded;
        assert_eq!(
            compute_trace_convention_inputs(&degraded, &some, &some).unwrap_err(),
            TraceInputError::DegradedIndex
        );
    }
}
