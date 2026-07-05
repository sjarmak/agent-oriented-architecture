use serde::{Deserialize, Serialize};

use aoa_gap::HeldOutProvenance;
use aoa_metrics::Confidence;

use crate::convention::{ConventionFamily, ScoringConvention};

/// The three independent facts that decide whether a repo may vote in R0.
///
/// A repo votes ONLY when it is high-confidence (SCIP-grade index), native-span
/// (its held-out suite is natively composed, not synthesized or reconstructed),
/// AND calibrated. A repo failing any one is excluded and does not contribute a
/// vote, per R-silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eligibility {
    /// Index confidence from `aoa-metrics`; only `Confidence::High` may vote.
    pub confidence: Confidence,
    /// Held-out provenance; only `NativeComposed` counts as native-span.
    pub native_span: HeldOutProvenance,
    /// Whether the repo's scoring is calibrated against external outcomes.
    pub calibrated: bool,
}

/// Trace-reach depth recorded when the oracle chain is unreachable from the
/// agent's trace footprint in the symbol graph: "deeper than any finite bound".
/// Any finite `max_depth` convention excludes a task carrying this value.
pub const UNREACHABLE_TRACE_REACH_DEPTH: u32 = u32::MAX;

/// The per-task scoring inputs an admissible convention may bound, tagged by
/// task shape so the two families can never be silently conflated.
///
/// - `Edit`: edit-shaped SDLC task. `edit_locality` (in `[0.0, 1.0]`) and
///   `mutation_depth` derive from >=2 accepted PR solutions and the mutation
///   surface (see `aoa-metrics`).
/// - `Answer`: answer-shaped comprehension task (pre-registered 2026-07-04,
///   bead aoa-dhk.1; see `docs/r0_runbook.md`). Per arm trial:
///   *trace-locality* = `|T ∩ O| / |T|` where `T` is the set of repo files the
///   agent's instrumented trace read or touched and `O` is the task's oracle
///   chain files, in `[0.0, 1.0]` (1.0 = every touched file was on the oracle
///   chain); *trace-reach depth* = the smallest `k` such that every oracle
///   chain file is within `k` undirected hops of the trace footprint in the
///   SCIP symbol graph ([`UNREACHABLE_TRACE_REACH_DEPTH`] when disconnected).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum ConventionInputs {
    Edit {
        edit_locality: f64,
        mutation_depth: u32,
    },
    Answer {
        repo_trace_locality: f64,
        harness_trace_locality: f64,
        repo_trace_reach_depth: u32,
        harness_trace_reach_depth: u32,
    },
}

impl ConventionInputs {
    /// The task shape these inputs belong to.
    pub fn family(&self) -> ConventionFamily {
        match self {
            ConventionInputs::Edit { .. } => ConventionFamily::Edit,
            ConventionInputs::Answer { .. } => ConventionFamily::Answer,
        }
    }
}

/// One identical-pair task with both held-out success bits and the scoring
/// inputs an admissible convention may re-weight.
///
/// `is_identical_pair` gates participation: only identical-pair tasks contribute
/// to either delta. The two success bits are the held-out (not visible) outcomes
/// under the two arms — repo arm (AOA migration, fixed harness) and harness arm
/// (swapped harness, fixed repo).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PairTask {
    pub task_id: u64,
    /// Whether the task is an identical pair across both arms. Non-paired tasks
    /// are excluded from both deltas.
    pub is_identical_pair: bool,
    /// Held-out success on the repo arm (AOA migration, fixed harness).
    pub repo_held_out_success: bool,
    /// Held-out success on the harness arm (swapped harness, fixed repo).
    pub harness_held_out_success: bool,
    /// The task-shape-tagged inputs the floor/ceiling and depth-k conventions
    /// admit or reject the task's contribution by.
    pub convention_inputs: ConventionInputs,
}

/// One fixed-seed replication of a repo's identical-pair tasks.
///
/// Determinism is checked by comparing the verdict computed from each `RepoRun`
/// across the `k_runs` replications. Variation is supplied by the caller (real
/// re-runs), never by an in-crate RNG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoRun {
    pub seed: u64,
    pub tasks: Vec<PairTask>,
}

/// All evidence for a single repo across its replicated runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoResult {
    pub repo_id: String,
    pub eligibility: Eligibility,
    /// One entry per fixed-seed replication. Must hold at least `k_runs`.
    pub runs: Vec<RepoRun>,
    /// Size of the held-out set backing this repo's evidence, for the power
    /// precondition.
    pub holdout_size: u32,
}

/// Policy thresholds and admissible conventions for the gate, all carried as
/// data so the verdict's preconditions are inspectable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FalsifyConfig {
    /// Determinism replication count; the verdict must be stable across this
    /// many fixed-seed runs. Must be >= 3.
    pub k_runs: u32,
    /// Minimum per-repo held-out size below which no significant verdict may be
    /// returned (power precondition).
    pub min_holdout_size: u32,
    /// Minimum aggregate effect size (mean `|repo_delta - harness_delta|` over
    /// eligible repos) below which the evidence is too weak to call either way
    /// and no significant verdict may be returned.
    pub min_effect_size: f64,
    /// The admissible scoring conventions the verdict must be invariant across.
    pub conventions: Vec<ScoringConvention>,
}

impl Default for FalsifyConfig {
    fn default() -> Self {
        Self {
            k_runs: 3,
            min_holdout_size: 20,
            min_effect_size: 0.0,
            conventions: ScoringConvention::admissible_edit(),
        }
    }
}

/// The complete input to the falsification gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FalsifyInput {
    pub repos: Vec<RepoResult>,
    pub config: FalsifyConfig,
}
