//! `aoa eval run`: post-process a completed codeprobe run into per-task AOA
//! metric records, emitted alongside codeprobe's own outcome scores.
//!
//! This command does NOT orchestrate an agent — codeprobe does. It consumes the
//! per-trial artifacts codeprobe persists under
//! `<run_dir>/<task_id>/{agent_output.txt, scoring.json}` (codeprobe
//! `core/executor.py::_save_task_artifacts`). For each task it runs the
//! trace-shim over the transcript, builds (or degrades) a symbol graph, joins
//! the task oracle, and computes the four process metrics plus the
//! reward-hacking gap.
//!
//! # Honest degradation (MVP boundaries)
//!
//! A codeprobe run retains neither the agent's patch nor a repo checkout, so:
//! - **`F_edit`** is reconstructed from `write.committed` span targets in the
//!   trace — the writes the transcript confirms actually landed. A prose-only
//!   trial has no writes, so edit-locality is degenerate — never fabricated.
//!   Attempted, failed, denied, and blocked writes stay in the trace but are
//!   not edits, so none of them inflates `F_edit`.
//! - **the symbol graph** needs an explicit `--scip-index` or `--repo`; absent
//!   one it degrades to zero weight (R0-ineligible), recorded in
//!   `graph_degrade_reason` rather than failing silently.
//! - **`visible_success`** has no independent signal in `scoring.json`, so it
//!   mirrors `held_out_success` and the record carries `visible_unobserved =
//!   true`. It must not be read as a real visible pass.
//! - **`invariant_set` (`I_t`)** is populated only from a SCIP index; for the
//!   best-effort/degraded tiers it is empty, making invariant-discoverability
//!   vacuous.
//! - **edit-locality** requires ≥2 accepted solutions; with fewer it is reported
//!   `null` with a reason (`InsufficientAcceptedSolutions`), never invented.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use aoa_bench::{discover_tasks, leg_pass, load_task};
use aoa_codeprobe_shim::parse_transcript_file;
use aoa_gap::{
    compute_gap, BehavioralSignal, GapOutcome, HeldOutProvenance, InsufficientDataNote, RunResult,
    TaskOutcome,
};
use aoa_metrics::{
    compute_edit_locality, compute_invariant_discoverability, compute_mutation_surface,
    compute_retrieval_locality, compute_subtree_metrics, discover_partition, ConditionedOn,
    Confidence, EditLocality, IndexQuality, InvariantDiscoverability, MetricError, MetricInputRef,
    MutationSurface, RetrievalLocality, SubtreeMetrics, SubtreePartition, TransformMap,
    WorkspaceSource,
};
use aoa_scip_graph::{build_symbol_graph, degraded, IndexSource, IndexedRepo};
use aoa_trace::Trace;

use crate::cli::EvalRunArgs;
use crate::commands::fsutil::load_json_capped;
use crate::output::{escape_terminal, print_human, print_json};

/// Mutation-surface reachability depth and retrieval cutoff. Fixed to the value
/// the metric crate's own integration tests exercise; not yet a CLI knob (YAGNI).
const DEFAULT_K: u32 = 2;

/// The subset of codeprobe's `scoring.json` this post-processor reads.
///
/// Deliberately NOT `aoa_bench::DualScoring`: that type hard-requires
/// `scorer_family == "dual_composite"` and reads the per-leg fields, whereas
/// `eval run` accepts any codeprobe scorer and reads the top-level composite
/// (see the module doc on `visible_unobserved`). The two decode different fields
/// of the same file on purpose; the pass *rule* is shared so they cannot drift.
#[derive(Debug, Deserialize)]
struct Scoring {
    #[serde(default)]
    score: f64,
    /// Present for binary scorers; preferred over the score threshold.
    passed: Option<bool>,
}

impl Scoring {
    fn held_out_success(&self) -> bool {
        // `#[serde(default)]` on `score` makes the no-signal case
        // indistinguishable from a genuine 0.0, so `leg_pass` never returns
        // `None` here. Making that absence loud is tracked as aoa-vme7.
        leg_pass(self.passed, Some(self.score)).unwrap_or(false)
    }
}

#[derive(Debug, Serialize)]
struct EvalRunReport {
    run_dir: String,
    /// Trials that produced a record (excludes failed trials, counted separately).
    record_count: usize,
    error_count: usize,
    /// The run's held-out behavioral signal: one observation per record,
    /// counted against the behavioral-signal window (aoa-d6t.23).
    behavioral_signal: BehavioralSignal,
    /// Present when the run is below the window: the per-record metrics are
    /// real per-trial measurements, but the run supplies too little held-out
    /// signal to stand as repo-level behavioral evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    insufficient_data: Option<InsufficientDataNote>,
    /// Present only when a multi-member workspace was detected under the
    /// partition root (`--subtree-root`, defaulting to `--repo`; aoa-d6t.26,
    /// aoa-d6t.32). Additive: absent, the schema is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    subtree_partition: Option<SubtreePartitionInfo>,
    records: Vec<TaskRecord>,
    errors: Vec<TaskError>,
}

/// How the repo was partitioned into subtrees, for JSON consumers.
#[derive(Debug, Serialize)]
struct SubtreePartitionInfo {
    source: WorkspaceSource,
    /// Member dirs relative to the repo root, lexicographic.
    members: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TaskError {
    task_id: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct TaskRecord {
    task_id: String,
    conditioned_on: ConditionedOn,
    held_out_success: bool,
    /// Only held-out passes count: a visible pass that fails held-out is `false`.
    counted_as_success: bool,
    /// `visible_success` was NOT independently observed in the codeprobe run; it
    /// mirrors `held_out_success`. Do not read it as a real visible pass.
    visible_unobserved: bool,
    held_out_provenance: HeldOutProvenance,
    graph_quality: IndexQuality,
    confidence: Confidence,
    weight: f64,
    repo_eligible_for_r0: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph_degrade_reason: Option<String>,
    /// Count of non-fatal shim warnings (e.g. non-JSON transcript lines); a
    /// nonzero value flags a possibly-truncated or corrupt transcript.
    transcript_warnings: usize,
    retrieval_locality: RetrievalLocality,
    invariant_discoverability: InvariantDiscoverability,
    mutation_surface: MutationSurface,
    /// `null` when fewer than two accepted solutions were mined — see
    /// `edit_locality_unavailable` for the reason. Never fabricated.
    edit_locality: Option<EditLocality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edit_locality_unavailable: Option<String>,
    /// Per-subtree rows (aoa-d6t.26), present only when a multi-member
    /// workspace was detected. Repo-wide fields above are unaffected.
    #[serde(skip_serializing_if = "Option::is_none")]
    subtree_metrics: Option<Vec<SubtreeMetrics>>,
    gap: GapOutcome,
}

/// Post-process a codeprobe run directory.
pub fn run(args: &EvalRunArgs) -> Result<i32> {
    // Build the (single) graph source once: a codeprobe run is one repo/config
    // arm, and `--scip-index`/`--repo` name a single source. Absent either, the
    // graph degrades — loudly, via `degrade_reason`, not silently.
    let indexed = build_graph(args);
    if let Some(reason) = &indexed.degrade_reason {
        eprintln!("warning: {reason}; all records will score weight=0.0 (R0-ineligible)");
    }

    // Per-subtree scoping (aoa-d6t.26): automatic when the partition root
    // (--subtree-root, defaulting to the --repo checkout) is a multi-member
    // workspace. The mode switch is logged, never silent.
    let partition = detect_partition(args);

    let task_ids = discover_tasks(&args.codeprobe_run)?;

    let mut records = Vec::new();
    let mut errors = Vec::new();
    for task_id in task_ids {
        let task_dir = args.codeprobe_run.join(&task_id);
        match process_task(&task_id, &task_dir, args, &indexed, partition.as_ref()) {
            Ok(record) => records.push(record),
            // Fail loud for THIS trial — reported, never silently skipped — and
            // keep processing the rest of the batch.
            Err(e) => errors.push(TaskError {
                task_id,
                error: format!("{e:#}"),
            }),
        }
    }

    let behavioral_signal = BehavioralSignal::from_observations(records.len());
    let report = EvalRunReport {
        run_dir: args.codeprobe_run.display().to_string(),
        record_count: records.len(),
        error_count: errors.len(),
        insufficient_data: behavioral_signal.insufficient_data(),
        behavioral_signal,
        subtree_partition: partition.as_ref().map(|p| {
            let mut members = p.members().to_vec();
            members.sort_unstable();
            SubtreePartitionInfo {
                source: p.source(),
                members,
            }
        }),
        records,
        errors,
    };

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&render_human(&report));
    }

    // Any failed trial makes the command exit non-zero so CI / downstream R0
    // experiments notice, without discarding the records that did compute.
    Ok(i32::from(report.error_count > 0))
}

/// Build the symbol graph from the explicit source, or a logged degraded graph.
fn build_graph(args: &EvalRunArgs) -> IndexedRepo {
    match (&args.scip_index, &args.repo) {
        (Some(index_path), _) => build_symbol_graph(IndexSource::Scip { index_path }),
        (None, Some(repo_dir)) => build_symbol_graph(IndexSource::BestEffort { repo_dir }),
        (None, None) => degraded(Some(
            "no graph source: pass --scip-index <file> or --repo <dir> for a weighted graph"
                .to_string(),
        )),
    }
}

/// Detect the subtree partition of the checkout, when a partition root exists.
///
/// The root is `--subtree-root` when given (aoa-d6t.32: the only source on
/// `--scip-index` runs, an explicit override otherwise), else the `--repo`
/// checkout. Returns `Some` only for a multi-member workspace — the only case
/// where per-subtree rows add signal — and logs the automatic mode switch. A
/// discovery failure (malformed manifest) is logged and falls back to
/// repo-wide reporting rather than aborting the run. An explicit
/// `--subtree-root` that yields no partition (missing directory, or no
/// multi-member workspace manifest) is likewise surfaced before falling back:
/// the user asked for it, so dropping it must never be silent. The automatic
/// `--repo` path stays quiet in that case — no flag was dropped.
fn detect_partition(args: &EvalRunArgs) -> Option<SubtreePartition> {
    let explicit = args.subtree_root.as_deref();
    let root = explicit.or(args.repo.as_deref())?;
    match discover_partition(root) {
        Ok(partition) if partition.is_partitioned() => {
            eprintln!(
                "per-subtree metrics enabled: {} workspace members detected via {} in {}",
                partition.members().len(),
                partition.source().label(),
                root.display()
            );
            Some(partition)
        }
        Ok(_) => {
            if let Some(root) = explicit {
                let reason = if root.is_dir() {
                    "no multi-member workspace manifest found"
                } else {
                    "not a directory"
                };
                eprintln!(
                    "warning: --subtree-root {}: {reason}; reporting repo-wide metrics only",
                    root.display()
                );
            }
            None
        }
        Err(e) => {
            eprintln!("warning: subtree discovery failed ({e}); reporting repo-wide metrics only");
            None
        }
    }
}

/// Build one task's metric record, or fail loud for this trial.
fn process_task(
    task_id: &str,
    task_dir: &Path,
    args: &EvalRunArgs,
    indexed: &IndexedRepo,
    partition: Option<&SubtreePartition>,
) -> Result<TaskRecord> {
    let transcript = task_dir.join("agent_output.txt");
    let shim = parse_transcript_file(&transcript)
        .with_context(|| format!("trace-shim failed on {}", transcript.display()))?;
    let trace = shim.trace;
    // A nonzero warning count flags a possibly-truncated/corrupt transcript; it
    // is surfaced on the record rather than dropped.
    let transcript_warnings = shim.warnings.len();

    let scoring_path = task_dir.join("scoring.json");
    let scoring: Scoring = load_json_capped(&scoring_path, "scoring")?;
    let held_out_success = scoring.held_out_success();

    // Oracle: when `--tasks` is given the task dir MUST load (fail loud); without
    // it we proceed oracle-less (empty gold set, no held-out provenance -> gap
    // unavailable).
    let task = match &args.tasks {
        Some(tasks_dir) => Some(
            load_task(tasks_dir.join(task_id))
                .with_context(|| format!("failed to load task {task_id} oracle"))?,
        ),
        None => None,
    };
    let gold_set = task
        .as_ref()
        .map(|t| t.gold_set().clone())
        .unwrap_or_default();
    let accepted_solutions = task
        .as_ref()
        .map(|t| t.accepted_solution_files())
        .unwrap_or_default();

    let edited_files = edited_files_from_trace(&trace);

    // The symbol graph and invariant set are task-invariant for the run — borrow
    // the single shared copy rather than cloning it into a fresh `MetricInput`
    // every task. Per-task fields (trace, gold set, edits, accepted solutions)
    // are owned on this stack frame and borrowed into the same view.
    let transform = TransformMap::default();
    let input = MetricInputRef {
        trace: &trace,
        gold_set: &gold_set,
        // I_t comes only from a SCIP index; empty (vacuous) otherwise.
        invariant_set: &indexed.invariant_set,
        transform: &transform,
        edited_files: &edited_files,
        accepted_solutions: &accepted_solutions,
        graph: &indexed.graph,
        k: DEFAULT_K,
        held_out_success,
    };

    // Edit-locality needs ≥2 accepted solutions; surface the shortfall rather
    // than fail the whole record. The match is intentionally exhaustive on
    // `MetricError`'s sole variant: a future variant must become a compile error
    // here so it is handled deliberately, never silently nulled.
    let (edit_locality, edit_locality_unavailable) = match compute_edit_locality(input) {
        Ok(e) => (Some(e), None),
        Err(MetricError::InsufficientAcceptedSolutions(n)) => (
            None,
            Some(format!("insufficient accepted solutions: {n} (need ≥2)")),
        ),
    };

    // The reward-hacking gap from codeprobe's oracle, built through the bench
    // bridge so provenance-stamping lives in one place. visible mirrors held-out
    // (visible_unobserved; see module docs). Oracle-less tasks carry provenance
    // `None`, which `compute_gap` reports as `Unavailable`.
    let run_result = match &task {
        Some(t) => t.to_run_result(held_out_success, held_out_success),
        None => RunResult {
            tasks: vec![TaskOutcome {
                visible_success: held_out_success,
                held_out_success,
            }],
            held_out_provenance: HeldOutProvenance::None,
            canaries: Vec::new(),
        },
    };
    let provenance = run_result.held_out_provenance;
    let gap = compute_gap(&run_result)
        .with_context(|| format!("gap computation failed for {task_id}"))?;

    let quality = input.graph.quality;
    Ok(TaskRecord {
        task_id: task_id.to_string(),
        conditioned_on: ConditionedOn::HeldOut,
        held_out_success,
        counted_as_success: held_out_success,
        visible_unobserved: true,
        held_out_provenance: provenance,
        graph_quality: quality,
        confidence: quality.confidence(),
        weight: quality.weight(),
        repo_eligible_for_r0: quality.eligible_for_r0(),
        graph_degrade_reason: indexed.degrade_reason.clone(),
        transcript_warnings,
        retrieval_locality: compute_retrieval_locality(input),
        invariant_discoverability: compute_invariant_discoverability(input),
        mutation_surface: compute_mutation_surface(input),
        edit_locality,
        edit_locality_unavailable,
        subtree_metrics: partition.map(|p| compute_subtree_metrics(input, p)),
        gap,
    })
}

/// `F_edit`: the files the agent actually edited, from `write.committed` span
/// `path` targets. A trial with no landed writes yields an empty set.
///
/// Counts only confirmed successful mutations. `write.blocked` used to be
/// included here, which was plainly wrong — a write the policy gate denied
/// never touched the file, so it inflated `F_edit` and depressed the edit
/// -locality inflation ratios that derive from it. `write.attempt` is excluded
/// for the same reason one step earlier: it records intent before execution.
fn edited_files_from_trace(trace: &Trace) -> BTreeSet<String> {
    trace
        .spans
        .iter()
        .filter(|s| s.span_type.is_confirmed_mutation())
        .filter_map(|s| s.attributes.get("path").and_then(|v| v.as_str()))
        .map(|p| p.to_string())
        .collect()
}

/// Short human label for the index quality tier (the JSON uses the serde form).
fn quality_label(quality: IndexQuality) -> &'static str {
    match quality {
        IndexQuality::Scip => "scip",
        IndexQuality::BestEffort => "best_effort",
        IndexQuality::Degraded => "degraded",
    }
}

fn render_human(report: &EvalRunReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "aoa eval run: {} record(s), {} error(s) from {}",
        report.record_count, report.error_count, report.run_dir
    );
    for r in &report.records {
        let gap = match r.gap.gap() {
            Some(g) => format!("{g:+.4}"),
            None => "unavailable".to_string(),
        };
        // `edit_locality_unavailable` is always set when `edit_locality` is None,
        // so a missing reason falls back rather than implying a fourth state.
        let edit = match &r.edit_locality {
            Some(e) => format!(
                "floor {:.2} / ceiling {:.2}",
                e.floor_inflation, e.ceiling_inflation
            ),
            None => r
                .edit_locality_unavailable
                .clone()
                .unwrap_or_else(|| "n/a".to_string()),
        };
        // `task_id` is a directory name from an untrusted run dir: escape it so a
        // crafted name cannot inject terminal control sequences into the output.
        let _ = writeln!(
            out,
            "  {:<28} held_out={} weight={:.1} graph={} gap={} edit=[{}]",
            escape_terminal(&r.task_id),
            r.held_out_success,
            r.weight,
            quality_label(r.graph_quality),
            gap,
            edit
        );
        // Per-subtree rows (aoa-d6t.26): indented under their task record.
        for row in r.subtree_metrics.iter().flatten() {
            let first = match row.retrieval_locality.tool_calls_to_first_relevant_artifact {
                Some(n) => n.to_string(),
                None => "-".to_string(),
            };
            // `mutation_unavailable` is always set when the numbers are None,
            // mirroring the `edit_locality_unavailable` fallback above.
            let mutation = match (row.mutation_surface, row.mutation_leakage) {
                (Some(surface), Some(leakage)) => {
                    format!("mutation_surface={surface} mutation_leakage={leakage}")
                }
                _ => format!(
                    "mutation=[{}]",
                    row.mutation_unavailable.as_deref().unwrap_or("n/a")
                ),
            };
            let _ = writeln!(
                out,
                "    subtree {:<24} spans={} edits={} first_relevant={} {}",
                escape_terminal(&row.subtree),
                row.attributed_span_count,
                row.edited_file_count,
                first,
                mutation
            );
        }
    }
    for e in &report.errors {
        let _ = writeln!(
            out,
            "  ERROR {:<26} {}",
            escape_terminal(&e.task_id),
            e.error
        );
    }
    if let Some(note) = &report.insufficient_data {
        let _ = writeln!(out, "{}", note.render_line(&report.behavioral_signal));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_trace::{Span, SpanSource, SpanType};
    use serde_json::Value;

    /// `F_edit` must stay non-empty for a transcript whose write succeeded.
    ///
    /// This is the guard on the repoint to `write.committed`. Nothing else
    /// would catch getting it wrong: an empty `F_edit` does not error, it flows
    /// into `compute_edit_locality` and reports `floor_inflation: 0.0` — a
    /// well-formed measurement claiming perfect edit locality for every trial.
    /// A silent, plausible, entirely wrong number is the worst failure this
    /// pipeline can produce, so the assertion is pinned against a real
    /// codeprobe artifact rather than a synthesized trace.
    #[test]
    fn f_edit_counts_the_landed_edit_in_a_real_codeprobe_transcript() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/codeprobe_run/native-consensus-001/agent_output.txt");
        let parsed = aoa_codeprobe_shim::parse_transcript_file(&fixture).expect("fixture parses");

        let edited = edited_files_from_trace(&parsed.trace);
        assert_eq!(
            edited.into_iter().collect::<Vec<_>>(),
            vec!["src/widget/config.py"],
            "the transcript's successful Edit must reach F_edit"
        );
    }

    fn write_span(seq: u64, span_type: SpanType, path: &str) -> Span {
        Span {
            span_type,
            source: SpanSource::Native,
            seq,
            attributes: [("path".to_string(), Value::String(path.into()))]
                .into_iter()
                .collect(),
        }
    }

    /// The other half of the same guard: every write the transcript reports as
    /// anything but a landed edit stays out of `F_edit`. `write.blocked` used to
    /// be counted here, which inflated it with files that were never touched.
    #[test]
    fn f_edit_excludes_writes_that_did_not_land() {
        let spans = [
            SpanType::WriteAttempt,
            SpanType::WriteBlocked,
            SpanType::WriteFailed,
            SpanType::WriteDenied,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, span_type)| write_span(i as u64, span_type, &format!("f{i}.rs")))
        .collect();

        assert!(
            edited_files_from_trace(&Trace { spans }).is_empty(),
            "no write in this trace landed, so F_edit is empty"
        );
    }
}
