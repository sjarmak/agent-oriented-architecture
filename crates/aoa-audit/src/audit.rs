use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_budget::{count_budget, resolve_closure, Config};
use aoa_metrics::{compute_mutation_surface, IndexQuality, MetricInput, SymbolGraph, TransformMap};
use aoa_trace::Trace;

use crate::error::AuditError;
use crate::planes::missing_planes;
use crate::punch::{rank, MeasuredCost, PunchItem};
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
/// mirroring the inspectable-defaults discipline of `aoa-gap`'s thresholds.
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
pub fn audit(repo: &Path, cfg: &AuditConfig) -> Result<AuditReport, AuditError> {
    let mut items = Vec::new();

    if let Some(item) = context_budget_item(repo, cfg)? {
        items.push(item);
    }
    items.push(mutation_surface_item(cfg));
    items.extend(plane_items(repo));
    items.extend(structure_items(repo, cfg.size_outlier_k)?);

    rank(&mut items);
    Ok(AuditReport::new(items))
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
        tier: Tier::Tier2,
        measured_cost: MeasuredCost::new(overflow as u64, "tokens over ceiling"),
        plane: None,
    }))
}

/// Emit the mutation-surface punch item. Cost = count of writable files
/// reachable within depth k (the writable blast radius is the actionable
/// number).
fn mutation_surface_item(cfg: &AuditConfig) -> PunchItem {
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

    PunchItem {
        title: format!("writable mutation surface within depth {}", cfg.k),
        tier: Tier::Tier2,
        measured_cost: MeasuredCost::new(
            surface.writable_reachable as u64,
            "writable files reachable",
        ),
        plane: None,
    }
}

/// One punch item per missing enforcement plane, tier mapped from the plane.
/// Cost = 1 missing plane (a real count: the plane is absent).
fn plane_items(repo: &Path) -> Vec<PunchItem> {
    missing_planes(repo)
        .into_iter()
        .map(|plane| PunchItem {
            title: format!("missing enforcement plane: {}", plane.label()),
            tier: plane.tier(),
            measured_cost: MeasuredCost::new(1, "missing plane"),
            plane: Some(plane),
        })
        .collect()
}
