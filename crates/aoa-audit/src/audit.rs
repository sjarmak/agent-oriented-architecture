use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aoa_budget::{count_budget, resolve_closure, Config};
use aoa_construct::BehavioralSignal;
use aoa_metrics::{
    compute_metrics, discover_partition, IndexQuality, MetricInput, MetricRecord, SubtreePartition,
    SymbolGraph, TransformMap,
};
use aoa_trace::Trace;
use serde::{Deserialize, Serialize};

use crate::error::AuditError;
use crate::liveness::{enforcement_liveness, EnforcementLiveness};
use crate::planes::missing_planes;
use crate::punch::{rank, FindingKind, MeasuredCost, PunchItem};
use crate::report::AuditReport;
use crate::structure::structure_items;
use crate::tier::{EnforcementPlane, Tier};

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
    /// Explicit same-task context for each observe session. Ambient session
    /// files do not carry invariant or accepted-solution evidence, so absence
    /// excludes the session rather than fabricating complete metric inputs.
    pub live_metric_contexts: BTreeMap<String, LiveMetricContext>,
    /// Multiplier for the module-size outlier check: a source file longer than
    /// `size_outlier_k ×` the repo's own median source-file line count is
    /// counted. Documented, overridable; never an absolute size threshold.
    pub size_outlier_k: f64,
    /// Window for the enforcement-liveness question: only live logs appended to
    /// at or after this instant count as evidence the plane is running. `None`
    /// asks it over the repository's whole history.
    pub enforcement_since: Option<SystemTime>,
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
            live_metric_contexts: BTreeMap::new(),
            size_outlier_k: DEFAULT_SIZE_OUTLIER_K,
            enforcement_since: None,
        }
    }
}

/// Evidence that cannot be inferred from an ambient trace and must be supplied
/// by the task producer for the named session.
#[derive(Debug, Clone, Default)]
pub struct LiveMetricContext {
    pub task_id: String,
    pub invariant_set: BTreeSet<String>,
    pub accepted_solutions: Vec<BTreeSet<String>>,
    pub transform: TransformMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveExclusionReason {
    NoLandedEdit,
    TaskContextMissing,
    InvariantEvidenceMissing,
    AcceptedSolutionsMissing,
    SymbolGraphMissing,
    MetricComputationFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LiveObservationState {
    Measured { metrics: Box<MetricRecord> },
    Excluded { reason: LiveExclusionReason },
}

/// One observe-captured session's inspectable metric outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveMetricObservation {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub held_out_edits: BTreeSet<String>,
    pub state: LiveObservationState,
}

/// Run a read-only audit of `repo`. Builds a ranked, tiered punch-list grounded
/// in measured numbers: the context-file token closure (aoa-budget), the
/// mutation-surface proxy (aoa-metrics), structural enforcement-plane checks,
/// and the code-structure family (navigability anchors, module-size outliers —
/// born Tier-3/advisory). Writes nothing.
///
/// Greenfield/cold-start precondition: each observe-captured session under
/// `.aoa/traces/` is evaluated as measured or explicitly excluded. Only
/// sessions with landed edits, a graph, and complete same-task context count
/// toward the behavioral window. Structural items need no traces and are
/// unaffected.
pub fn audit(repo: &Path, cfg: &AuditConfig) -> Result<AuditReport, AuditError> {
    let corpus = aoa_observe_shim::load_corpus(repo)?;
    let live_observations = live_metric_observations(&corpus.sessions, cfg);
    let measured: Vec<&MetricRecord> = live_observations
        .iter()
        .filter_map(|observation| match &observation.state {
            LiveObservationState::Measured { metrics } => Some(&**metrics),
            LiveObservationState::Excluded { .. } => None,
        })
        .collect();
    let signal = BehavioralSignal::from_observations(measured.len());

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
        items.extend(mutation_surface_item(&measured));
    }
    let liveness = enforcement_liveness(repo, cfg.enforcement_since);
    items.extend(plane_items(repo, &liveness));
    items.extend(structure_items(repo, cfg.size_outlier_k, &partition)?);

    rank(&mut items);
    Ok(AuditReport {
        subtree_discovery_warning,
        live_observations,
        enforcement_liveness: liveness,
        ..AuditReport::with_signal(items, signal)
    })
}

fn live_metric_observations(
    sessions: &[aoa_observe_shim::ObservedSession],
    cfg: &AuditConfig,
) -> Vec<LiveMetricObservation> {
    sessions
        .iter()
        .map(|session| {
            let edited_files = aoa_observe_shim::held_out_edits(&session.trace);
            let context = cfg.live_metric_contexts.get(&session.session_id);
            let state = match measure_live_session(session, &edited_files, context, cfg) {
                Ok(metrics) => LiveObservationState::Measured {
                    metrics: Box::new(metrics),
                },
                Err(reason) => LiveObservationState::Excluded { reason },
            };
            LiveMetricObservation {
                session_id: session.session_id.clone(),
                task_id: context
                    .filter(|context| !context.task_id.trim().is_empty())
                    .map(|context| context.task_id.clone()),
                held_out_edits: edited_files,
                state,
            }
        })
        .collect()
}

fn measure_live_session(
    session: &aoa_observe_shim::ObservedSession,
    edited_files: &BTreeSet<String>,
    context: Option<&LiveMetricContext>,
    cfg: &AuditConfig,
) -> Result<MetricRecord, LiveExclusionReason> {
    if edited_files.is_empty() {
        return Err(LiveExclusionReason::NoLandedEdit);
    }
    if cfg.graph.nodes.is_empty() || cfg.graph.quality == IndexQuality::Degraded {
        return Err(LiveExclusionReason::SymbolGraphMissing);
    }
    let context = context.ok_or(LiveExclusionReason::TaskContextMissing)?;
    if context.task_id.trim().is_empty() {
        return Err(LiveExclusionReason::TaskContextMissing);
    }
    if context.invariant_set.is_empty() {
        return Err(LiveExclusionReason::InvariantEvidenceMissing);
    }
    if context.accepted_solutions.len() < 2
        || context.accepted_solutions.iter().any(BTreeSet::is_empty)
    {
        return Err(LiveExclusionReason::AcceptedSolutionsMissing);
    }

    let input = MetricInput {
        trace: trace_before_first_write(&session.trace),
        gold_set: edited_files.clone(),
        invariant_set: context.invariant_set.clone(),
        transform: context.transform.clone(),
        edited_files: edited_files.clone(),
        accepted_solutions: context.accepted_solutions.clone(),
        graph: cfg.graph.clone(),
        k: cfg.k,
        held_out_success: true,
    };
    compute_metrics(input.as_view()).map_err(|_| LiveExclusionReason::MetricComputationFailed)
}

fn trace_before_first_write(trace: &Trace) -> Trace {
    let boundary = trace
        .spans
        .iter()
        .filter(|span| span.span_type.opens_write_boundary())
        .map(|span| span.seq)
        .min();
    Trace {
        spans: trace
            .spans
            .iter()
            .filter(|span| boundary.is_none_or(|seq| span.seq < seq))
            .cloned()
            .collect(),
    }
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

/// Emit the conservative maximum mutation surface across fully measured live
/// observations. The maximum is deterministic and keeps the largest observed
/// writable blast radius actionable; an empty measured set emits no item.
fn mutation_surface_item(metrics: &[&MetricRecord]) -> Option<PunchItem> {
    let surface = metrics
        .iter()
        .map(|record| &record.mutation_surface)
        .max_by_key(|surface| surface.writable_reachable)?;

    Some(PunchItem {
        title: format!("writable mutation surface within depth {}", surface.k),
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

/// One punch item per missing enforcement plane, tier mapped from the plane,
/// plus one for a runtime plane that is installed and emitting nothing.
///
/// Cost = 1 plane in both cases (a real count, not a score). The silent item is
/// what keeps an inert plane off the pass side of the ledger: `missing_planes`
/// sees an installed hook set and says nothing, so before this the only two
/// readings were "missing" and silence-as-health. An installed-but-silent plane
/// is missing *evidence*, and the audit reports it at the plane's own tier
/// rather than withholding it (the aoa-xo8y0 rule, one layer over).
///
/// The two never both fire for one plane: `missing_planes` reports the runtime
/// plane only when the hook set is absent, and liveness reports silence only
/// when it is present.
fn plane_items(repo: &Path, liveness: &EnforcementLiveness) -> Vec<PunchItem> {
    let mut items: Vec<PunchItem> = missing_planes(repo)
        .into_iter()
        .map(|plane| PunchItem {
            title: format!("missing enforcement plane: {}", plane.label()),
            kind: FindingKind::MissingPlane,
            tier: plane.tier(),
            measured_cost: MeasuredCost::new(1, "missing plane"),
            plane: Some(plane),
            subtree: None,
        })
        .collect();

    if let Some(silence) = liveness.silence() {
        let plane = EnforcementPlane::RuntimeHook;
        items.push(PunchItem {
            title: format!(
                "enforcement plane installed but silent: {} ({})",
                plane.label(),
                silence.reason()
            ),
            kind: FindingKind::SilentPlane,
            tier: plane.tier(),
            measured_cost: MeasuredCost::new(1, "silent plane"),
            plane: Some(plane),
            subtree: None,
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_metric_trace_stops_before_the_first_write_boundary() {
        let trace: Trace = serde_json::from_str(
            r#"{"spans":[
                {"type":"file.read","source":"native","seq":1,"attributes":{"path":"before.rs"}},
                {"type":"write.attempt","source":"native","seq":2,"attributes":{"path":"edit.rs"}},
                {"type":"file.read","source":"native","seq":3,"attributes":{"path":"after.rs"}},
                {"type":"write.committed","source":"native","seq":4,"attributes":{"path":"edit.rs"}}
            ]}"#,
        )
        .expect("valid trace");

        let before = trace_before_first_write(&trace);

        assert_eq!(before.spans.len(), 1);
        assert_eq!(before.spans[0].seq, 1);
    }
}
