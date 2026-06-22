//! `aoa eval experiment`: build an R0 `FalsifyInput` from a codeprobe
//! experiment's paired config arms.
//!
//! R0 attributes a held-out delta to a *layer*. The experiment runs the SAME
//! mined tasks under two config arms and this builder joins them into the
//! paired-repo evidence `aoa falsify` scores:
//! - **repo arm** (`repo_arm`): the agent on the AOA-MIGRATED repo, fixed
//!   harness. Its held-out (ARTIFACT) leg becomes `PairTask.repo_held_out_success`.
//! - **harness arm** (`harness_arm`): a swapped agent/harness on the fixed
//!   baseline repo. Its held-out (ARTIFACT) leg becomes
//!   `PairTask.harness_held_out_success`.
//!
//! BOTH arms contribute their held-out (artifact) leg — this is NOT the r0b
//! mapping (artifact-vs-direct within one run). The two arms are two different
//! codeprobe configs; the *visible* (direct) leg plays no part in R0.
//!
//! # Honesty boundaries
//!
//! - **Eligibility is never fabricated.** `confidence` (SCIP-grade index) and
//!   `calibrated` are operator assertions, REQUIRED per repo in the manifest —
//!   no default toward eligible. `native_span` is derived from the mined task
//!   oracle (held-out provenance), never declared.
//! - **Convention inputs degrade to abstention, never to admitting defaults.**
//!   A task's `edit_locality`/`mutation_depth` need a per-repo symbol graph this
//!   builder does not construct (deferred to the live-scale work). Rather than
//!   emit the midpoint values that every admissible convention silently admits —
//!   which would let the R0' convention-invariance check *pass* on no evidence —
//!   the builder flags `convention_inputs_degraded` so `aoa falsify` abstains.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use aoa_bench::load_task;
use aoa_falsify::{
    is_eligible, Eligibility, FalsifyConfig, FalsifyInput, PairTask, RepoResult, RepoRun,
    ScoringConvention,
};
use aoa_gap::HeldOutProvenance;
use aoa_metrics::Confidence;

use crate::cli::ExperimentArgs;
use crate::commands::codeprobe::{aggregate_provenance, discover_tasks, DualScoring};
use crate::commands::fsutil::{read_to_string_capped, MAX_JSON_BYTES};
use crate::output::{print_human, print_json};

/// Sentinel convention inputs emitted while a per-repo symbol graph is not
/// constructed. They are NOT the admitting midpoint: the builder pairs them with
/// `convention_inputs_degraded` so the gate abstains rather than reads them as
/// real evidence.
const DEGRADED_EDIT_LOCALITY: f64 = 0.0;
const DEGRADED_MUTATION_DEPTH: u32 = 0;

// ---------------------------------------------------------------------------
// Manifest (operator-authored)
// ---------------------------------------------------------------------------

/// The whole build manifest.
#[derive(Debug, Deserialize)]
struct Manifest {
    /// Determinism replication count (>= 3); each repo must supply this many runs.
    k_runs: u32,
    /// Power precondition: minimum per-repo held-out size.
    min_holdout_size: u32,
    /// Power precondition: minimum aggregate effect size. Defaults to `0.0`,
    /// which disables the effect-size floor (every effect clears `>= 0.0`) —
    /// matching `aoa_falsify::FalsifyConfig::default`. Set it explicitly to make
    /// the power precondition bite.
    #[serde(default)]
    min_effect_size: f64,
    repos: Vec<RepoManifest>,
}

/// One repo's operator assertions and its per-seed arm run dirs.
#[derive(Debug, Deserialize)]
struct RepoManifest {
    repo_id: String,
    /// Operator assertion that the repo carries a SCIP-grade (high-confidence)
    /// index. REQUIRED — there is no safe default toward eligibility.
    confidence: ConfidenceDecl,
    /// Operator assertion that the repo's scoring is calibrated. REQUIRED.
    calibrated: bool,
    runs: Vec<RunManifest>,
}

/// One fixed-seed replication: the two arm run dirs over the same mined tasks.
#[derive(Debug, Deserialize)]
struct RunManifest {
    seed: u64,
    /// codeprobe config-label run dir for the AOA-migrated arm.
    repo_arm: PathBuf,
    /// codeprobe config-label run dir for the harness-swap arm.
    harness_arm: PathBuf,
}

/// Operator-declared index confidence. Spelled lowercase in the manifest.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConfidenceDecl {
    High,
    Low,
}

impl From<ConfidenceDecl> for Confidence {
    fn from(d: ConfidenceDecl) -> Self {
        match d {
            ConfidenceDecl::High => Confidence::High,
            ConfidenceDecl::Low => Confidence::Low,
        }
    }
}

// ---------------------------------------------------------------------------
// Build report (emitted alongside the FalsifyInput, consumed by `aoa falsify`)
// ---------------------------------------------------------------------------

/// One task dropped from a repo's identical-pair set, with the reason.
#[derive(Debug, Serialize)]
struct ExcludedTask {
    task_id: String,
    reason: String,
}

/// Per-repo build provenance: what was assembled and why.
#[derive(Debug, Serialize)]
struct RepoBuild {
    repo_id: String,
    identical_pairs: usize,
    holdout_size: u32,
    native_span: HeldOutProvenance,
    confidence: Confidence,
    calibrated: bool,
    /// Whether this repo satisfies the gate's eligibility predicate (high +
    /// native-composed + calibrated). Informational — the gate re-derives it.
    eligible: bool,
    excluded_tasks: Vec<ExcludedTask>,
    /// Per-repo build notes (e.g. seed-to-seed identical-pair instability).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

/// The build report. `convention_inputs_degraded` is the load-bearing flag
/// `aoa falsify --build-meta` reads to decide whether to abstain.
#[derive(Debug, Serialize)]
pub(crate) struct BuildReport {
    out_path: String,
    repo_count: usize,
    total_identical_pairs: usize,
    convention_inputs_degraded: bool,
    repos: Vec<RepoBuild>,
    notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/// One arm's held-out outcomes for one run, keyed by task id.
struct ArmOutcomes {
    /// task_id -> held-out (artifact-leg) success.
    held_out: BTreeMap<String, bool>,
    /// task_id -> the load error, for tasks that could not be read as a clean
    /// dual result (excluded, not fatal).
    excluded: BTreeMap<String, String>,
}

impl ArmOutcomes {
    /// Every trial discovered in this arm, regardless of whether it scored
    /// cleanly — the set used to detect presence mismatches across arms.
    fn discovered(&self) -> BTreeSet<String> {
        self.held_out
            .keys()
            .chain(self.excluded.keys())
            .cloned()
            .collect()
    }
}

/// Read one arm run dir into per-task held-out outcomes. A task whose scoring is
/// missing/non-dual/errored is recorded as excluded (with reason), not fatal —
/// it simply cannot form a clean identical pair.
fn read_arm(run_dir: &Path) -> Result<ArmOutcomes> {
    let task_ids = discover_tasks(run_dir)
        .with_context(|| format!("failed to discover arm trials in {}", run_dir.display()))?;
    let mut held_out = BTreeMap::new();
    let mut excluded = BTreeMap::new();
    for task_id in task_ids {
        let scoring_path = run_dir.join(&task_id).join("scoring.json");
        match DualScoring::load(&scoring_path, &task_id).and_then(|s| s.held_out_success(&task_id))
        {
            Ok(success) => {
                held_out.insert(task_id, success);
            }
            Err(e) => {
                excluded.insert(task_id, format!("{e:#}"));
            }
        }
    }
    Ok(ArmOutcomes { held_out, excluded })
}

/// Assemble one repo's `RepoResult` over its fixed-seed runs, collecting the
/// per-repo build provenance.
fn build_repo(
    repo: &RepoManifest,
    tasks_dir: &Path,
    base_dir: &Path,
    k_runs: u32,
) -> Result<Option<(RepoResult, RepoBuild)>> {
    if (repo.runs.len() as u32) < k_runs {
        bail!(
            "repo {}: manifest supplies {} run(s) but k_runs is {}; each repo needs \
             at least k_runs fixed-seed replications",
            repo.repo_id,
            repo.runs.len(),
            k_runs
        );
    }

    let mut runs = Vec::with_capacity(repo.runs.len());
    // Exclusions are accumulated across ALL runs, deduped by task id (a task that
    // drops out in any seed is recorded once), so a seed-specific mismatch is
    // never silently swallowed.
    let mut excluded: BTreeMap<String, String> = BTreeMap::new();
    let mut representative_ids: Vec<String> = Vec::new();
    let mut pair_counts: Vec<usize> = Vec::with_capacity(repo.runs.len());

    for (run_index, run) in repo.runs.iter().enumerate() {
        // Arm paths in the manifest are resolved relative to the manifest file's
        // directory (an absolute path passes through `join` unchanged).
        let repo_arm = read_arm(&base_dir.join(&run.repo_arm))?;
        let harness_arm = read_arm(&base_dir.join(&run.harness_arm))?;

        // Identical-pair candidates: present in BOTH arms.
        let in_both: BTreeSet<&String> = repo_arm
            .held_out
            .keys()
            .filter(|id| harness_arm.held_out.contains_key(*id))
            .collect();

        // Record exclusions for THIS run: an un-clean dual result in either arm,
        // and presence mismatches (a trial that ran in one arm but not the other
        // is not an identical pair). Annotated with the seed so a seed-specific
        // drop is visible. `entry` dedupes across runs (first reason wins).
        let seed = run.seed;
        for (id, reason) in repo_arm.excluded.iter().chain(harness_arm.excluded.iter()) {
            excluded
                .entry(id.clone())
                .or_insert_with(|| format!("seed {seed}: {reason}"));
        }
        let repo_seen = repo_arm.discovered();
        let harness_seen = harness_arm.discovered();
        for id in repo_seen.difference(&harness_seen) {
            excluded.entry(id.clone()).or_insert_with(|| {
                format!("seed {seed}: absent from the harness arm — not an identical pair")
            });
        }
        for id in harness_seen.difference(&repo_seen) {
            excluded.entry(id.clone()).or_insert_with(|| {
                format!("seed {seed}: absent from the repo arm — not an identical pair")
            });
        }

        let mut ids: Vec<String> = in_both.into_iter().cloned().collect();
        ids.sort();

        let tasks: Vec<PairTask> = ids
            .iter()
            .enumerate()
            .map(|(idx, id)| PairTask {
                // The crate treats task_id as an opaque label; a stable per-repo
                // enumeration keeps it deterministic and inspectable.
                task_id: idx as u64,
                is_identical_pair: true,
                repo_held_out_success: repo_arm.held_out[id],
                harness_held_out_success: harness_arm.held_out[id],
                edit_locality: DEGRADED_EDIT_LOCALITY,
                mutation_depth: DEGRADED_MUTATION_DEPTH,
            })
            .collect();

        pair_counts.push(tasks.len());
        if run_index == 0 {
            representative_ids = ids;
        }
        runs.push(RepoRun {
            seed: run.seed,
            tasks,
        });
    }

    let min_pairs = pair_counts.iter().copied().min().unwrap_or(0);
    let max_pairs = pair_counts.iter().copied().max().unwrap_or(0);

    // A repo with no identical pairs in some run cannot supply consistent
    // evidence; drop it (loudly noted) rather than emit empty runs that score as
    // zero-delta.
    if min_pairs == 0 || representative_ids.is_empty() {
        return Ok(None);
    }

    let mut repo_notes = Vec::new();
    if min_pairs != max_pairs {
        repo_notes.push(format!(
            "identical-pair count varies across seeds ({min_pairs}..{max_pairs}); \
             holdout_size uses the minimum and the determinism check may flag instability"
        ));
    }

    // Repo-level held-out provenance from the representative run's identical-pair
    // task oracles. The tasks dir is shared across arms (same mined tasks), so
    // provenance is a task property, identical across arms by construction.
    let mut provenances = Vec::with_capacity(representative_ids.len());
    for id in &representative_ids {
        let task = load_task(tasks_dir.join(id)).with_context(|| {
            format!(
                "failed to load task {id} oracle from {}",
                tasks_dir.display()
            )
        })?;
        provenances.push(task.held_out_provenance());
    }
    let native_span = aggregate_provenance(&provenances)
        .with_context(|| format!("repo {}: held-out provenance", repo.repo_id))?;

    let confidence: Confidence = repo.confidence.into();
    let holdout_size = min_pairs as u32;
    let eligibility = Eligibility {
        confidence,
        native_span,
        calibrated: repo.calibrated,
    };
    // Reuse the gate's own predicate so the build report's `eligible` flag cannot
    // drift from the eligibility rule `aoa falsify` actually applies.
    let eligible = is_eligible(&eligibility);

    let excluded_tasks = excluded
        .into_iter()
        .map(|(task_id, reason)| ExcludedTask { task_id, reason })
        .collect();
    let build = RepoBuild {
        repo_id: repo.repo_id.clone(),
        identical_pairs: min_pairs,
        holdout_size,
        native_span,
        confidence,
        calibrated: repo.calibrated,
        eligible,
        excluded_tasks,
        notes: repo_notes,
    };
    let result = RepoResult {
        repo_id: repo.repo_id.clone(),
        eligibility,
        runs,
        holdout_size,
    };
    Ok(Some((result, build)))
}

/// Build the `FalsifyInput` and the build report from the manifest.
fn build(
    manifest: &Manifest,
    tasks_dir: &Path,
    base_dir: &Path,
) -> Result<(FalsifyInput, BuildReport)> {
    if manifest.repos.is_empty() {
        bail!("manifest declares no repos");
    }

    let mut repos = Vec::new();
    let mut repo_builds = Vec::new();
    let mut notes = Vec::new();

    for repo in &manifest.repos {
        match build_repo(repo, tasks_dir, base_dir, manifest.k_runs)? {
            Some((result, build)) => {
                repos.push(result);
                repo_builds.push(build);
            }
            None => notes.push(format!(
                "repo {}: no identical-pair tasks across both arms; excluded from the input",
                repo.repo_id
            )),
        }
    }

    let total_identical_pairs = repo_builds.iter().map(|r| r.identical_pairs).sum();
    notes.push(
        "convention inputs (edit_locality, mutation_depth) are degraded: this builder does not \
         construct a per-repo symbol graph, so the R0' convention-invariance check cannot be \
         exercised and `aoa falsify` will abstain (inconclusive). Wiring real per-task convention \
         inputs is the live-scale follow-up."
            .to_string(),
    );

    let config = FalsifyConfig {
        k_runs: manifest.k_runs,
        min_holdout_size: manifest.min_holdout_size,
        min_effect_size: manifest.min_effect_size,
        conventions: ScoringConvention::admissible_default(),
    };
    let input = FalsifyInput { repos, config };
    let report = BuildReport {
        out_path: String::new(), // filled by the caller once the path is known
        repo_count: repo_builds.len(),
        total_identical_pairs,
        convention_inputs_degraded: true,
        repos: repo_builds,
        notes,
    };
    Ok((input, report))
}

/// Path the build report is written to: the `--out` path with a `.build.json`
/// extension (e.g. `falsify_input.json` -> `falsify_input.build.json`).
fn build_report_path(out: &Path) -> PathBuf {
    out.with_extension("build.json")
}

/// Run `aoa eval experiment`.
pub(crate) fn run(args: &ExperimentArgs) -> Result<i32> {
    let raw = read_to_string_capped(&args.manifest, MAX_JSON_BYTES)
        .with_context(|| format!("failed to read manifest {}", args.manifest.display()))?;
    let manifest: Manifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse manifest {}", args.manifest.display()))?;

    let base_dir = args.manifest.parent().unwrap_or_else(|| Path::new("."));
    let (input, mut report) = build(&manifest, &args.tasks, base_dir)?;

    let input_json = serde_json::to_string_pretty(&input)?;
    std::fs::write(&args.out, &input_json)
        .with_context(|| format!("failed to write {}", args.out.display()))?;

    report.out_path = args.out.display().to_string();
    let report_path = build_report_path(&args.out);
    let report_json = serde_json::to_string_pretty(&report)?;
    std::fs::write(&report_path, format!("{report_json}\n"))
        .with_context(|| format!("failed to write {}", report_path.display()))?;

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&render_human(&report, &report_path));
    }
    Ok(0)
}

fn render_human(report: &BuildReport, report_path: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "aoa eval experiment: built {} repo(s), {} identical pair(s) -> {}",
        report.repo_count, report.total_identical_pairs, report.out_path,
    );
    for r in &report.repos {
        // repo_id is operator-authored free text; escape it before display to
        // match the hardening applied to task ids below.
        let _ = writeln!(
            out,
            "  {:<24} pairs={} holdout={} provenance={:?} confidence={:?} calibrated={} eligible={}",
            r.repo_id.escape_debug(),
            r.identical_pairs,
            r.holdout_size,
            r.native_span,
            r.confidence,
            r.calibrated,
            r.eligible,
        );
        for note in &r.notes {
            let _ = writeln!(out, "      note: {note}");
        }
        for ex in &r.excluded_tasks {
            let _ = writeln!(
                out,
                "      excluded {}: {}",
                ex.task_id.escape_debug(),
                ex.reason
            );
        }
    }
    if report.convention_inputs_degraded {
        let _ = writeln!(
            out,
            "  convention_inputs_degraded=true -> the verdict will abstain (inconclusive)",
        );
    }
    let _ = writeln!(out, "  build report: {}", report_path.display());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_report_path_swaps_extension() {
        assert_eq!(
            build_report_path(Path::new("falsify_input.json")),
            PathBuf::from("falsify_input.build.json")
        );
    }

    #[test]
    fn confidence_decl_maps_to_metrics_confidence() {
        assert_eq!(Confidence::from(ConfidenceDecl::High), Confidence::High);
        assert_eq!(Confidence::from(ConfidenceDecl::Low), Confidence::Low);
    }

    #[test]
    fn run_rejects_oversized_manifest() {
        let dir = std::env::temp_dir().join(format!("aoa-experiment-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("manifest.json");
        std::fs::write(&manifest, vec![b'x'; (MAX_JSON_BYTES + 1) as usize]).unwrap();

        let args = ExperimentArgs {
            manifest,
            tasks: dir.clone(),
            out: dir.join("falsify_input.json"),
            json: false,
        };
        let err = run(&args).unwrap_err();
        assert!(format!("{err:#}").contains("byte cap"), "got: {err:#}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
