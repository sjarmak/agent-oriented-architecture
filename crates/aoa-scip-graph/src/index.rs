use std::collections::BTreeSet;
use std::path::Path;

use aoa_metrics::{IndexQuality, SymbolGraph};

use crate::best_effort::index_best_effort;
use crate::scip::index_with_scip;

/// A real target repo indexed into the artifacts the metric extractors consume.
///
/// The [`SymbolGraph`] carries nodes, edges, the writable subset, and the R15
/// [`IndexQuality`]. `gold_set` is `G_t` and `invariant_set` is `I_t`, both as
/// base-repo symbol names (anchor them with an `aoa_metrics::TransformMap`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedRepo {
    pub graph: SymbolGraph,
    pub gold_set: BTreeSet<String>,
    pub invariant_set: BTreeSet<String>,
}

/// Which index source to read for a repo.
#[derive(Debug, Clone)]
pub enum IndexSource<'a> {
    /// A vendored SCIP-grade JSON index at the given path (high confidence).
    Scip { index_path: &'a Path },
    /// A best-effort AST/line scan rooted at the given repo directory.
    BestEffort { repo_dir: &'a Path },
}

/// Build a [`SymbolGraph`] (plus `G_t`/`I_t`) for a repo from the given source.
///
/// On a read or parse failure — or a source that produces no nodes — the repo is
/// reported as [`IndexQuality::Degraded`] rather than erroring, so a broken index
/// silently lowers weight and loses its R0 vote instead of aborting scoring. The
/// degraded outcome is the explicit fallback contract of this entry point; the
/// source-specific functions still surface their errors for callers that need
/// them.
pub fn build_symbol_graph(source: IndexSource<'_>) -> IndexedRepo {
    let indexed = match source {
        IndexSource::Scip { index_path } => index_with_scip(index_path),
        IndexSource::BestEffort { repo_dir } => index_best_effort(repo_dir),
    };

    match indexed {
        Ok(repo) if !repo.graph.nodes.is_empty() => repo,
        _ => degraded(),
    }
}

/// The degraded sentinel: an empty graph marked [`IndexQuality::Degraded`].
///
/// Per R15 this weighs zero and is ineligible for R0; `aoa-metrics` enforces
/// those consequences from the `quality` field alone.
pub fn degraded() -> IndexedRepo {
    IndexedRepo {
        graph: SymbolGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            writable: BTreeSet::new(),
            quality: IndexQuality::Degraded,
        },
        gold_set: BTreeSet::new(),
        invariant_set: BTreeSet::new(),
    }
}
