use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use aoa_budget::{count_budget, resolve_closure, Config};
use aoa_construct::BehavioralSignal;
use aoa_metrics::{
    compute_mutation_surface, discover_partition, IndexQuality, MetricInput, SubtreePartition,
    SymbolGraph, TransformMap,
};
use aoa_trace::Trace;

use crate::error::AuditError;
use crate::planes::missing_planes;
use crate::punch::{rank, FindingKind, MeasuredCost, PunchItem};
use crate::report::AuditReport;
use crate::structure::structure_items;
use crate::tier::Tier;

/// The reference encoding used for the context-budget probe. o200k_base loads
/// without network access and is the pinned reference encoding of aoa-budget.
const AUDIT_TARGET_TOKENIZER: &str = "o200k_base";

/// Default context-file token ceiling. Closures over this contribute an
/// oversized-context punch item whose cost is the measured overflow.
const DEFAULT_CONTEXT_CEILING: usize = 2_000;

/// Default mutation-surface reachability depth.
const DEFAULT_MUTATION_K: u32 = 2;

/// Default module-size outlier multiplier: a source file longer than this many
/// times the repo's *own* median source-file line count is counted as an
/// outlier. Self-calibrating against the repo's distribution rather than an
/// absolute size, so it asserts no external best-practice. Overridable per run,
/// mirroring the inspectable-defaults discipline of `aoa-construct`'s gating
/// thresholds.
const DEFAULT_SIZE_OUTLIER_K: f64 = 4.0;

/// Configuration for a read-only audit run. Every field is data the caller
/// supplies; the audit makes no semantic judgments of its own.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    /// Root context document to resolve the token closure from, relative to the
    /// repo (e.g. `AGENTS.md`). `None` skips the context-budget probe.
    pub context_root: Option<PathBuf>,
    /// Token ceiling for the context closure.
    pub ceiling: usize,
    /// Target tokenizer name passed to aoa-budget.
    pub target: String,
    /// The symbol graph used for the mutation-surface proxy. Modeled in-crate;
    /// the audit never shells out to a real SCIP indexer.
    pub graph: SymbolGraph,
    /// Mutation-surface reachability depth.
    pub k: u32,
    /// Trace used to ground the retrieval-locality proxy.
    pub trace: Trace,
    /// Gold artifact symbols anchoring the retrieval-locality proxy.
    pub gold_set: BTreeSet<String>,
    /// Multiplier for the module-size outlier check: a source file longer than
    /// `size_outlier_k ×` the repo's own median source-file line count is
    /// counted. Documented, overridable; never an absolute size threshold.
    pub size_outlier_k: f64,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            context_root: Some(PathBuf::from("AGENTS.md")),
            ceiling: DEFAULT_CONTEXT_CEILING,
            target: AUDIT_TARGET_TOKENIZER.to_string(),
            graph: SymbolGraph {
                nodes: Vec::new(),
                edges: Vec::new(),
                writable: BTreeSet::new(),
                node_paths: BTreeMap::new(),
                quality: IndexQuality::BestEffort,
            },
            k: DEFAULT_MUTATION_K,
            trace: Trace { spans: Vec::new() },
            gold_set: BTreeSet::new(),
            size_outlier_k: DEFAULT_SIZE_OUTLIER_K,
        }
    }
}

/// Run a read-only audit of `repo`. Builds a ranked, tiered punch-list grounded
/// in measured numbers: the context-file token closure (aoa-budget), the
/// mutation-surface proxy (aoa-metrics), structural enforcement-plane checks,
/// and the code-structure family (navigability anchors, module-size outliers —
/// born Tier-3/advisory). Writes nothing.
///
/// Greenfield/cold-start precondition (aoa-d6t.23): the observe-captured trace
/// corpus under `.aoa/traces/` is counted first. Below the behavioral-signal
/// window the behavioral punch item (mutation surface) is withheld — a repo
/// with no held-out signal must report InsufficientData on
/// [`AuditReport::insufficient_data`], never a fabricated score. Crossing the
/// window is not enough on its own: the item also needs a real symbol graph to
/// measure against (see [`mutation_surface_item`]). Structural items need no
/// traces and are unaffected.
pub fn audit(repo: &Path, cfg: &AuditConfig) -> Result<AuditReport, AuditError> {
    let corpus = aoa_observe_shim::load_corpus(repo)?;
    let signal = BehavioralSignal::from_observations(corpus.observations());

    let mut items = Vec::new();

    // The workspace partition scopes path-carrying structure findings to
    // their member subtree. A manifest that exists but cannot be used never
    // costs the operator the punch-list: attribution degrades to repo-wide
    // findings with the failure surfaced on the report — never silently, and
    // never a guess. Absence of a manifest is the implicit-root partition.
    let (partition, subtree_discovery_warning) = match discover_partition(repo) {
        Ok(partition) => (partition, None),
        Err(e) => (
            SubtreePartition::implicit_root(repo),
            Some(format!(
                "subtree discovery failed ({e}); findings are repo-wide"
            )),
        ),
    };

    if let Some(item) = context_budget_item(repo, cfg)? {
        items.push(item);
    }
    if signal.is_sufficient() {
        items.extend(mutation_surface_item(cfg));
    }
    items.extend(plane_items(repo));
    items.extend(structure_items(repo, cfg.size_outlier_k, &partition)?);

    rank(&mut items);
    Ok(AuditReport {
        subtree_discovery_warning,
        ..AuditReport::with_signal(items, signal)
    })
}

/// Measure the context-file token closure and, when over the ceiling, emit an
/// oversized-context punch item whose cost is the token overflow.
fn context_budget_item(repo: &Path, cfg: &AuditConfig) -> Result<Option<PunchItem>, AuditError> {
    let Some(root_rel) = &cfg.context_root else {
        return Ok(None);
    };
    let root = repo.join(root_rel);
    if !root.exists() {
        return Ok(None);
    }

    let closure = resolve_closure(&root)?;
    let report = count_budget(&closure, &cfg.target, &Config::warn_first(cfg.ceiling))?;
    let overflow = report.gating_target_tokens.saturating_sub(cfg.ceiling);
    if overflow == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: format!(
            "context closure from {} exceeds the token ceiling",
            root_rel.display()
        ),
        kind: FindingKind::ContextBudget,
        tier: Tier::Tier2,
        measured_cost: MeasuredCost::new(overflow as u64, "tokens over ceiling"),
        plane: None,
        subtree: None,
    }))
}

/// Emit the mutation-surface punch item. Cost = count of writable files
/// reachable within depth k (the writable blast radius is the actionable
/// number). `None` when the symbol graph carries no nodes: with nothing
/// indexed there is no measurement, and "0 writable files reachable" would be
/// a fabricated claim, not a measured one (aoa-d6t.23) — the same skip-probe
/// discipline as [`context_budget_item`] without its context root.
fn mutation_surface_item(cfg: &AuditConfig) -> Option<PunchItem> {
    if cfg.graph.nodes.is_empty() {
        return None;
    }
    let input = MetricInput {
        trace: cfg.trace.clone(),
        gold_set: cfg.gold_set.clone(),
        invariant_set: BTreeSet::new(),
        transform: TransformMap::default(),
        edited_files: BTreeSet::new(),
        accepted_solutions: Vec::new(),
        graph: cfg.graph.clone(),
        k: cfg.k,
        held_out_success: true,
    };

    let surface = compute_mutation_surface(input.as_view());

    Some(PunchItem {
        title: format!("writable mutation surface within depth {}", cfg.k),
        kind: FindingKind::MutationSurface,
        tier: Tier::Tier2,
        measured_cost: MeasuredCost::new(
            surface.writable_reachable as u64,
            "writable files reachable",
        ),
        plane: None,
        subtree: None,
    })
}

/// One punch item per missing enforcement plane, tier mapped from the plane.
/// Cost = 1 missing plane (a real count: the plane is absent).
fn plane_items(repo: &Path) -> Vec<PunchItem> {
    missing_planes(repo)
        .into_iter()
        .map(|plane| PunchItem {
            title: format!("missing enforcement plane: {}", plane.label()),
            kind: FindingKind::MissingPlane,
            tier: plane.tier(),
            measured_cost: MeasuredCost::new(1, "missing plane"),
            plane: Some(plane),
            subtree: None,
        })
        .collect()
}
