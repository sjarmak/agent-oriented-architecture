use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use aoa_trace::Trace;

/// Build quality of the symbol index backing a repo's metrics (R15).
///
/// `Scip` is a precise, SCIP-grade index and yields `high-confidence` records.
/// `BestEffort` is a heuristic index and yields `low-confidence` records.
/// `Degraded` is an empty or unusable index: it lowers score weight to zero,
/// never improves mutation-surface, and disqualifies the repo from R0 voting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexQuality {
    Scip,
    BestEffort,
    Degraded,
}

/// Confidence label carried on every metric record (R15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    High,
    Low,
}

impl IndexQuality {
    /// SCIP-grade indexes are high-confidence; everything else is low-confidence.
    pub fn confidence(self) -> Confidence {
        match self {
            IndexQuality::Scip => Confidence::High,
            IndexQuality::BestEffort | IndexQuality::Degraded => Confidence::Low,
        }
    }

    /// Per-quality score weight: precise index counts fully, best-effort counts
    /// at half, a degraded index contributes nothing (R-silent).
    pub fn weight(self) -> f64 {
        match self {
            IndexQuality::Scip => 1.0,
            IndexQuality::BestEffort => 0.5,
            IndexQuality::Degraded => 0.0,
        }
    }

    /// Whether a repo carrying this index quality may vote in R0 (R-silent).
    pub fn eligible_for_r0(self) -> bool {
        !matches!(self, IndexQuality::Degraded)
    }
}

/// A SCIP-style symbol graph modeled directly in-crate.
///
/// `nodes` are symbol or file identifiers, `edges` are directed reachability
/// links `(from, to)`, and `writable` is the subset of nodes the agent is
/// permitted to mutate. `quality` drives confidence labeling and R-silent gating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolGraph {
    pub nodes: Vec<String>,
    pub edges: Vec<(String, String)>,
    pub writable: BTreeSet<String>,
    pub quality: IndexQuality,
}

/// The base-repo-to-migrated-repo symbol map used to anchor gold sets.
///
/// Gold artifacts are named by their stable base-repo symbol; the trace
/// references the migrated names. `base_to_migrated` bridges the two so that a
/// renamed symbol still resolves to the artifact the trace actually touched.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TransformMap {
    pub base_to_migrated: BTreeMap<String, String>,
}

impl TransformMap {
    /// Anchor a set of base-repo symbols to the migrated names the trace uses.
    /// Symbols absent from the map pass through unchanged.
    pub fn anchor(&self, base_symbols: &BTreeSet<String>) -> BTreeSet<String> {
        base_symbols
            .iter()
            .map(|s| {
                self.base_to_migrated
                    .get(s)
                    .cloned()
                    .unwrap_or_else(|| s.clone())
            })
            .collect()
    }
}

/// The complete input to the metric extractors for a single task run.
#[derive(Debug, Clone)]
pub struct MetricInput {
    /// The instrumented tool-call trace for the run.
    pub trace: Trace,
    /// Base-repo gold artifact symbols `G_t` (anchored via `transform`).
    pub gold_set: BTreeSet<String>,
    /// Base-repo invariant artifact symbols `I_t` (anchored via `transform`).
    pub invariant_set: BTreeSet<String>,
    /// Base-to-migrated symbol map.
    pub transform: TransformMap,
    /// The files changed by the agent's final patch (`F_edit`).
    pub edited_files: BTreeSet<String>,
    /// Two or more accepted solution file-sets; their intersection is the floor
    /// and their union is the ceiling for edit-locality inflation.
    pub accepted_solutions: Vec<BTreeSet<String>>,
    /// The SCIP-style symbol graph.
    pub graph: SymbolGraph,
    /// Mutation-surface reachability depth bound.
    pub k: u32,
    /// Whether the run passed the held-out check. Records are always conditioned
    /// on held-out success; a visible pass with a held-out fail is `false` here.
    pub held_out_success: bool,
}
