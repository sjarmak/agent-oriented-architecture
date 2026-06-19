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
//! - **`F_edit`** is reconstructed from `write.attempt`/`write.blocked` span
//!   targets in the trace. A prose-only trial has no writes, so edit-locality is
//!   degenerate — never fabricated.
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

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use aoa_bench::load_task;
use aoa_codeprobe_shim::parse_transcript_file;
use aoa_gap::{compute_gap, GapOutcome, HeldOutProvenance, RunResult, TaskOutcome};
use aoa_metrics::{
    compute_edit_locality, compute_invariant_discoverability, compute_mutation_surface,
    compute_retrieval_locality, ConditionedOn, Confidence, EditLocality, IndexQuality,
    InvariantDiscoverability, MetricError, MetricInput, MutationSurface, RetrievalLocality,
    TransformMap,
};
use aoa_scip_graph::{build_symbol_graph, degraded, IndexSource, IndexedRepo};
use aoa_trace::{SpanType, Trace};

use crate::cli::EvalRunArgs;
use crate::output::{print_human, print_json};

/// Mutation-surface reachability depth and retrieval cutoff. Fixed to the value
/// the metric crate's own integration tests exercise; not yet a CLI knob (YAGNI).
const DEFAULT_K: u32 = 2;

/// `scoring.json` `score` at or above this counts as a held-out pass when the
/// explicit `passed` boolean is absent (exact-match scorers emit 0.0/1.0).
const SCORE_PASS_THRESHOLD: f64 = 1.0;

/// The subset of codeprobe's `scoring.json` this post-processor reads.
#[derive(Debug, Deserialize)]
struct Scoring {
    #[serde(default)]
    score: f64,
    /// Present for binary scorers; preferred over the score threshold.
    passed: Option<bool>,
}

impl Scoring {
    fn held_out_success(&self) -> bool {
        self.passed.unwrap_or(self.score >= SCORE_PASS_THRESHOLD)
    }
}

#[derive(Debug, Serialize)]
struct EvalRunReport {
    run_dir: String,
    /// Trials that produced a record (excludes failed trials, counted separately).
    record_count: usize,
    error_count: usize,
    records: Vec<TaskRecord>,
    errors: Vec<TaskError>,
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

    let task_ids = discover_tasks(&args.codeprobe_run)?;

    let mut records = Vec::new();
    let mut errors = Vec::new();
    for task_id in task_ids {
        let task_dir = args.codeprobe_run.join(&task_id);
        match process_task(&task_id, &task_dir, args, &indexed) {
            Ok(record) => records.push(record),
            // Fail loud for THIS trial — reported, never silently skipped — and
            // keep processing the rest of the batch.
            Err(e) => errors.push(TaskError {
                task_id,
                error: format!("{e:#}"),
            }),
        }
    }

    let report = EvalRunReport {
        run_dir: args.codeprobe_run.display().to_string(),
        record_count: records.len(),
        error_count: errors.len(),
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

/// List the `<task_id>` subdirectories of the run dir that look like trials.
///
/// A trial dir is identified by EITHER per-trial artifact: codeprobe always
/// writes `scoring.json` but writes `agent_output.txt` only when the agent
/// produced stdout. Keying on either means a trial that is missing its
/// transcript is still discovered — and then fails loud in [`process_task`] —
/// rather than being silently skipped.
fn discover_tasks(run_dir: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(run_dir)
        .with_context(|| format!("failed to read codeprobe run dir {}", run_dir.display()))?;

    let mut task_ids: Vec<String> = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", run_dir.display()))?;
        // `DirEntry::file_type` does NOT follow symlinks: a symlinked directory
        // must not pull in per-trial artifacts from outside the run tree.
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat entry in {}", run_dir.display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let dir = entry.path();
        if dir.join("scoring.json").is_file() || dir.join("agent_output.txt").is_file() {
            task_ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    task_ids.sort();

    if task_ids.is_empty() {
        bail!(
            "no task trials found under {}: expected <task_id>/ subdirs with scoring.json \
             or agent_output.txt (point --codeprobe-run at a run's config-label directory)",
            run_dir.display()
        );
    }
    Ok(task_ids)
}

/// Build one task's metric record, or fail loud for this trial.
fn process_task(
    task_id: &str,
    task_dir: &Path,
    args: &EvalRunArgs,
    indexed: &IndexedRepo,
) -> Result<TaskRecord> {
    let transcript = task_dir.join("agent_output.txt");
    let shim = parse_transcript_file(&transcript)
        .with_context(|| format!("trace-shim failed on {}", transcript.display()))?;
    let trace = shim.trace;
    // A nonzero warning count flags a possibly-truncated/corrupt transcript; it
    // is surfaced on the record rather than dropped.
    let transcript_warnings = shim.warnings.len();

    let scoring_path = task_dir.join("scoring.json");
    let scoring_raw = std::fs::read_to_string(&scoring_path)
        .with_context(|| format!("failed to read {}", scoring_path.display()))?;
    let scoring: Scoring = serde_json::from_str(&scoring_raw)
        .with_context(|| format!("failed to parse {}", scoring_path.display()))?;
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
        .map(|t| t.accepted_solutions.clone())
        .unwrap_or_default();

    let edited_files = edited_files_from_trace(&trace);

    let input = MetricInput {
        trace,
        gold_set,
        // I_t comes only from a SCIP index; empty (vacuous) otherwise.
        invariant_set: indexed.invariant_set.clone(),
        transform: TransformMap::default(),
        edited_files,
        accepted_solutions,
        graph: indexed.graph.clone(),
        k: DEFAULT_K,
        held_out_success,
    };

    // Edit-locality needs ≥2 accepted solutions; surface the shortfall rather
    // than fail the whole record. The match is intentionally exhaustive on
    // `MetricError`'s sole variant: a future variant must become a compile error
    // here so it is handled deliberately, never silently nulled.
    let (edit_locality, edit_locality_unavailable) = match compute_edit_locality(&input) {
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
        retrieval_locality: compute_retrieval_locality(&input),
        invariant_discoverability: compute_invariant_discoverability(&input),
        mutation_surface: compute_mutation_surface(&input),
        edit_locality,
        edit_locality_unavailable,
        gap,
    })
}

/// `F_edit`: the files the agent wrote, from `write.attempt`/`write.blocked`
/// span `path` targets. A trial with no writes yields an empty set.
fn edited_files_from_trace(trace: &Trace) -> BTreeSet<String> {
    trace
        .spans
        .iter()
        .filter(|s| matches!(s.span_type, SpanType::WriteAttempt | SpanType::WriteBlocked))
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
            r.task_id.escape_debug(),
            r.held_out_success,
            r.weight,
            quality_label(r.graph_quality),
            gap,
            edit
        );
    }
    for e in &report.errors {
        let _ = writeln!(out, "  ERROR {:<26} {}", e.task_id.escape_debug(), e.error);
    }
    out
}
