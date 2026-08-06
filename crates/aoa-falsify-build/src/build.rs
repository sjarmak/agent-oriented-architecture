//! Build an R0 `FalsifyInput` from a codeprobe
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
//! - **Eligibility is never fabricated.** `confidence` (SCIP-grade index), a
//!   pinned repository commit, both arm configs, and typed calibration evidence
//!   are REQUIRED per repo in the manifest. Calibration eligibility follows
//!   only from a complete artifact whose conclusion is `calibrated`;
//!   `native_span` is derived from the mined task oracle.
//! - **Convention inputs are real or the gate abstains.** For answer-shaped
//!   repos (`task_shape: "answer"` + `scip_index`) the builder computes real
//!   per-task trace-locality/trace-reach inputs by joining trial traces, the
//!   task's oracle chain, and the SCIP symbol graph (module [`answer`]).
//! - **Missing evidence remains data.** Every discovered candidate produces a
//!   content-addressed `Measured` or typed `Excluded` observation. A pair is
//!   admitted only when both arm observations are measured; edit-shaped repos
//!   remain excluded until their four required metrics can be computed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result as AnyResult};

use crate::answer::AnswerContext;
use crate::error::FalsifyBuildError;
use crate::evidence::{build_observation, read_artifact, read_calibration, RepoEvidence};
use crate::exposure::resolve_exposure;
use crate::manifest::{Manifest, RepoManifest, TaskShape};
use crate::report::{BuildReport, DroppedRepo, ExcludedTask, RepoBuild};
use aoa_bench::{
    aggregate_provenance, discover_tasks, load_task, ArmIdentity, CalibrationConclusion,
    MeasurementMetricsV1, MeasurementObservationV1, MeasurementStateV1, TrialScoring,
};
use aoa_falsify::{
    is_eligible, ConventionInputs, Eligibility, FalsifyConfig, FalsifyInput, PairTask, RepoResult,
    RepoRun, ScoringConvention,
};
use aoa_metrics::Confidence;

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
fn read_arm(run_dir: &Path) -> AnyResult<ArmOutcomes> {
    let task_ids = discover_tasks(run_dir)
        .with_context(|| format!("failed to discover arm trials in {}", run_dir.display()))?;
    let mut held_out = BTreeMap::new();
    let mut excluded = BTreeMap::new();
    for task_id in task_ids {
        match TrialScoring::load(run_dir, &task_id).and_then(|s| s.held_out_outcome()) {
            Ok(Some(success)) => {
                held_out.insert(task_id, success);
            }
            Ok(None) => {
                excluded.insert(
                    task_id,
                    "scoring carries neither `passed` nor `score`: no held-out signal".to_string(),
                );
            }
            Err(e) => {
                // `e` is a `BenchError`, not an `anyhow::Error`: its
                // `#[error(..)]` already inlines `{source}`, so there is no
                // chain to walk and no alternate flag to set.
                excluded.insert(task_id, e.to_string());
            }
        }
    }
    Ok(ArmOutcomes { held_out, excluded })
}

/// One repo's build outcome: included in the input, or dropped (no identical
/// pairs) with its per-task exclusion reasons preserved.
enum RepoOutcome {
    Included(Box<(RepoResult, RepoBuild, Vec<MeasurementObservationV1>)>),
    Dropped(DroppedRepo, Vec<MeasurementObservationV1>),
}

/// Assemble one repo's result and provenance. `answer_ctx` is present exactly
/// for answer-shaped repos and computes each pair's real convention inputs.
fn build_repo(
    repo: &RepoManifest,
    tasks_dir: &Path,
    base_dir: &Path,
    k_runs: u32,
    mut answer_ctx: Option<AnswerContext>,
    answer_context_error: Option<String>,
) -> AnyResult<RepoOutcome> {
    repo.repo_commit
        .validate()
        .with_context(|| format!("repo {}: invalid repo_commit", repo.repo_id))?;
    let repo_evidence = RepoEvidence {
        calibration: read_calibration(&base_dir.join(&repo.calibration_artifact)),
        repo_config: read_artifact(&base_dir.join(&repo.repo_arm_config)),
        harness_config: read_artifact(&base_dir.join(&repo.harness_arm_config)),
        index: repo
            .scip_index
            .as_ref()
            .map(|path| read_artifact(&base_dir.join(path))),
    };
    if (repo.runs.len() as u32) < k_runs {
        bail!(
            "repo {}: manifest supplies {} run(s) but k_runs is {}; each repo needs \
             at least k_runs fixed-seed replications",
            repo.repo_id,
            repo.runs.len(),
            k_runs
        );
    }

    // R0 determinism evidence is only meaningful across K INDEPENDENT runs.
    // Reject a manifest that reuses a seed or an arm run directory across runs:
    // a reused seed or dir reads the same draw twice, so "stable across K runs"
    // would be vacuously true rather than real replication evidence (aoa-g2g5).
    let mut seen_seeds: BTreeSet<u64> = BTreeSet::new();
    let mut seen_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for run in &repo.runs {
        if !seen_seeds.insert(run.seed) {
            bail!(
                "repo {}: seed {} is used by more than one run; each of the k_runs \
                 replications must use a distinct seed",
                repo.repo_id,
                run.seed
            );
        }
        for dir in [&run.repo_arm, &run.harness_arm] {
            // Compare RESOLVED directories, not raw manifest spellings: two runs
            // that name the same physical dir via `.`/`..`/symlink aliases would
            // otherwise pass this guard and read identical outcomes, restoring the
            // vacuous replication it exists to reject. The dir is read via
            // `base_dir.join` below, so resolve the same way; canonicalize needs
            // the dir to exist, so fall back to the lexical join when it does not
            // (a missing dir fails loudly later in read_arm).
            let resolved = base_dir.join(dir);
            let key = resolved.canonicalize().unwrap_or(resolved);
            if !seen_dirs.insert(key) {
                bail!(
                    "repo {}: run directory {} is used by more than one run/arm; each \
                     replication must read a distinct run directory",
                    repo.repo_id,
                    dir.display()
                );
            }
        }
    }

    let mut runs = Vec::with_capacity(repo.runs.len());
    // Exclusions are accumulated across ALL runs, deduped by task id (a task that
    // drops out in any seed is recorded once), so a seed-specific mismatch is
    // never silently swallowed.
    let mut excluded: BTreeMap<String, String> = BTreeMap::new();
    // Each run's admitted identical-pair ids (already sorted). Retained for EVERY
    // run, not just run 0, so the cross-run identity check below can compare them.
    let mut admitted_per_run: Vec<Vec<String>> = Vec::with_capacity(repo.runs.len());
    let mut observations = Vec::new();

    for run in &repo.runs {
        // Arm paths in the manifest are resolved relative to the manifest file's
        // directory (an absolute path passes through `join` unchanged).
        let repo_arm_dir = base_dir.join(&run.repo_arm);
        let harness_arm_dir = base_dir.join(&run.harness_arm);
        let repo_arm = read_arm(&repo_arm_dir)?;
        let harness_arm = read_arm(&harness_arm_dir)?;

        // Observation candidates are the union across arms. A missing
        // counterpart still emits an Excluded observation.
        let candidates: BTreeSet<String> = repo_arm
            .discovered()
            .union(&harness_arm.discovered())
            .cloned()
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

        let mut tasks: Vec<PairTask> = Vec::with_capacity(candidates.len());
        let mut admitted_ids: Vec<String> = Vec::with_capacity(candidates.len());
        for id in candidates {
            let repo_outcome = repo_arm
                .held_out
                .get(&id)
                .copied()
                .map(Ok)
                .or_else(|| repo_arm.excluded.get(&id).cloned().map(Err));
            let harness_outcome = harness_arm
                .held_out
                .get(&id)
                .copied()
                .map(Ok)
                .or_else(|| harness_arm.excluded.get(&id).cloned().map(Err));
            let (repo_observation, repo_diagnostic) = build_observation(
                repo,
                seed,
                ArmIdentity::Repo,
                &repo_arm_dir,
                &id,
                repo_outcome,
                tasks_dir,
                &repo_evidence,
                answer_ctx.as_mut(),
                answer_context_error.as_deref(),
            )?;
            let (harness_observation, harness_diagnostic) = build_observation(
                repo,
                seed,
                ArmIdentity::Harness,
                &harness_arm_dir,
                &id,
                harness_outcome,
                tasks_dir,
                &repo_evidence,
                answer_ctx.as_mut(),
                answer_context_error.as_deref(),
            )?;
            let convention_inputs = match (&repo_observation.state, &harness_observation.state) {
                (
                    MeasurementStateV1::Measured {
                        metrics: repo_metrics,
                        ..
                    },
                    MeasurementStateV1::Measured {
                        metrics: harness_metrics,
                        ..
                    },
                ) => match (&**repo_metrics, &**harness_metrics) {
                    (
                        MeasurementMetricsV1::Answer(repo_metrics),
                        MeasurementMetricsV1::Answer(harness_metrics),
                    ) => ConventionInputs::Answer {
                        repo_trace_locality: repo_metrics.trace_locality.value,
                        harness_trace_locality: harness_metrics.trace_locality.value,
                        repo_trace_reach_depth: repo_metrics.trace_reach_depth.value,
                        harness_trace_reach_depth: harness_metrics.trace_reach_depth.value,
                    },
                    _ => unreachable!("measured pair metrics share the manifest task shape"),
                },
                _ => {
                    let diagnostic = repo_diagnostic
                        .or(harness_diagnostic)
                        .unwrap_or_else(|| "pair observation excluded".to_string());
                    excluded
                        .entry(id.clone())
                        .or_insert_with(|| format!("seed {seed}: {diagnostic}"));
                    observations.push(repo_observation);
                    observations.push(harness_observation);
                    continue;
                }
            };
            let (repo_success, harness_success) =
                match (&repo_observation.state, &harness_observation.state) {
                    (
                        MeasurementStateV1::Measured {
                            held_out_success: repo,
                            ..
                        },
                        MeasurementStateV1::Measured {
                            held_out_success: harness,
                            ..
                        },
                    ) => (*repo, *harness),
                    _ => unreachable!("convention inputs require measured observations"),
                };
            tasks.push(PairTask {
                task_id: id.clone(),
                repo_observation_id: repo_observation.id.to_string(),
                harness_observation_id: harness_observation.id.to_string(),
                is_identical_pair: true,
                repo_held_out_success: repo_success,
                harness_held_out_success: harness_success,
                convention_inputs,
            });
            admitted_ids.push(id);
            observations.push(repo_observation);
            observations.push(harness_observation);
        }

        admitted_per_run.push(admitted_ids);
        runs.push(RepoRun {
            seed: run.seed,
            tasks,
        });
    }

    let min_pairs = admitted_per_run.iter().map(Vec::len).min().unwrap_or(0);

    // A repo with no identical pairs in some run cannot supply consistent
    // evidence; drop it (loudly noted, per-task reasons preserved) rather than
    // emit empty runs that score as zero-delta. This precedes the identity check
    // below: a zero-pair run is an ABSENCE of evidence, handled by dropping the
    // repo, not a misaligned-identity integrity failure.
    if min_pairs == 0 {
        let candidate_pairs = excluded.len();
        return Ok(RepoOutcome::Dropped(
            DroppedRepo {
                repo_id: repo.repo_id.clone(),
                candidate_pairs,
                pair_yield: 0.0,
                excluded_tasks: excluded
                    .into_iter()
                    .map(|(task_id, reason)| ExcludedTask { task_id, reason })
                    .collect(),
            },
            observations,
        ));
    }

    // Every run must admit the SAME identical-pair task identities, not merely
    // the same count. Positional `PairTask.task_id`s and the run-indexed
    // determinism check (aoa_falsify::verdict) both assume run i's task j is
    // run 0's task j; equal-count-but-different-membership runs silently break
    // that alignment, so the determinism evidence would compare mismatched
    // tasks. Fail loud with the missing/extra ids (aoa-g2g5).
    let representative = &admitted_per_run[0];
    for (run_index, ids) in admitted_per_run.iter().enumerate().skip(1) {
        if ids != representative {
            let reference: BTreeSet<&String> = representative.iter().collect();
            let this: BTreeSet<&String> = ids.iter().collect();
            let missing: Vec<&String> = reference.difference(&this).copied().collect();
            let extra: Vec<&String> = this.difference(&reference).copied().collect();
            bail!(
                "repo {}: run {} (seed {}) admits a different identical-pair set than run 0 \
                 (missing {:?}, extra {:?}); determinism across runs requires identical task \
                 identities, not just equal counts",
                repo.repo_id,
                run_index,
                repo.runs[run_index].seed,
                missing,
                extra
            );
        }
    }

    // Repo-level held-out provenance from the representative run's identical-pair
    // task oracles. The tasks dir is shared across arms (same mined tasks), so
    // provenance is a task property, identical across arms by construction.
    let mut provenances = Vec::with_capacity(representative.len());
    for id in representative {
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

    // Derived from the persisted ledger, never asserted by the manifest: this is
    // the anti-leakage check the whole gate rests on, so a ledger that does not
    // describe this repo at this revision must fail the build rather than resolve
    // to a votable `Unexposed`. Read here, where the status is about to be stated
    // — a repo dropped for want of identical pairs states no eligibility facts at
    // all, and the cheap structural checks above should fail before any IO.
    let exposure = resolve_exposure(
        &base_dir.join(&repo.exposure_scan),
        &repo.repo_id,
        &repo.repo_commit,
    )?;

    let confidence: Confidence = repo.confidence.into();
    let holdout_size = min_pairs as u32;
    let calibrated = repo_evidence
        .calibration
        .value
        .as_ref()
        .is_some_and(|evidence| evidence.conclusion == CalibrationConclusion::Calibrated);
    let eligibility = Eligibility {
        confidence,
        native_span,
        calibrated,
        exposure: exposure.clone(),
    };
    // Reuse the gate's own predicate so the build report's `eligible` flag cannot
    // drift from the eligibility rule `aoa falsify` actually applies.
    let eligible = is_eligible(&eligibility);

    let candidate_pairs = min_pairs + excluded.len();
    let pair_yield = min_pairs as f64 / candidate_pairs as f64;
    let excluded_tasks = excluded
        .into_iter()
        .map(|(task_id, reason)| ExcludedTask { task_id, reason })
        .collect();
    let build = RepoBuild {
        repo_id: repo.repo_id.clone(),
        identical_pairs: min_pairs,
        candidate_pairs,
        pair_yield,
        holdout_size,
        native_span,
        confidence,
        calibrated,
        exposure,
        eligible,
        excluded_tasks,
    };
    let result = RepoResult {
        repo_id: repo.repo_id.clone(),
        eligibility,
        runs,
        holdout_size,
    };
    Ok(RepoOutcome::Included(Box::new((
        result,
        build,
        observations,
    ))))
}

/// Validate the manifest's task-shape declarations: one uniform shape per
/// manifest (the gate scores one convention family), `scip_index` required for
/// `answer` and rejected for `edit` (where it would silently do nothing).
fn validated_shape(manifest: &Manifest) -> AnyResult<TaskShape> {
    let shape = manifest.repos[0].task_shape;
    for repo in &manifest.repos {
        if repo.task_shape != shape {
            bail!(
                "manifest mixes task shapes ({:?} and {:?}); one experiment scores one task shape",
                shape,
                repo.task_shape
            );
        }
        match (repo.task_shape, &repo.scip_index) {
            (TaskShape::Answer, None) => bail!(
                "repo {}: task_shape \"answer\" requires scip_index (the vendored SCIP JSON \
                 index the trace-locality/trace-reach inputs are derived from)",
                repo.repo_id
            ),
            (TaskShape::Edit, Some(_)) => bail!(
                "repo {}: scip_index is only read for task_shape \"answer\"; declare the shape \
                 or drop the index",
                repo.repo_id
            ),
            _ => {}
        }
    }
    Ok(shape)
}

/// Build the `FalsifyInput` and the build report from the manifest.
fn build_inner(
    manifest: &Manifest,
    tasks_dir: &Path,
    base_dir: &Path,
) -> AnyResult<(FalsifyInput, BuildReport, Vec<MeasurementObservationV1>)> {
    manifest.validate_repo_inventory()?;
    if manifest.repos.is_empty() {
        bail!("manifest declares no repos");
    }
    let shape = validated_shape(manifest)?;

    let mut repos = Vec::new();
    let mut repo_builds = Vec::new();
    let mut dropped_repos = Vec::new();
    let mut notes = Vec::new();
    let mut observations = Vec::new();

    for repo in &manifest.repos {
        let (answer_ctx, answer_context_error) = match &repo.scip_index {
            // validated_shape guarantees: Some(index) <=> answer shape.
            Some(index) => {
                match AnswerContext::load(&repo.repo_id, &base_dir.join(index), tasks_dir) {
                    Ok(context) => (Some(context), None),
                    Err(error) => (None, Some(format!("{error:#}"))),
                }
            }
            None => (None, None),
        };
        match build_repo(
            repo,
            tasks_dir,
            base_dir,
            manifest.k_runs,
            answer_ctx,
            answer_context_error,
        )? {
            RepoOutcome::Included(included) => {
                let (result, build, repo_observations) = *included;
                repos.push(result);
                repo_builds.push(build);
                observations.extend(repo_observations);
            }
            RepoOutcome::Dropped(dropped, repo_observations) => {
                notes.push(format!(
                    "repo {}: no identical-pair tasks across both arms; excluded from the input \
                     (per-task reasons under dropped_repos)",
                    dropped.repo_id
                ));
                dropped_repos.push(dropped);
                observations.extend(repo_observations);
            }
        }
    }

    let total_identical_pairs = repo_builds.iter().map(|r| r.identical_pairs).sum();
    let (conventions, convention_inputs_degraded) = match shape {
        TaskShape::Edit => {
            notes.push(
                "convention inputs (edit_locality, mutation_depth) are degraded: no edit-task \
                 symbol-graph pipeline exists, so the R0' convention-invariance check cannot be \
                 exercised and `aoa falsify` will abstain (inconclusive). Answer-shaped repos \
                 (task_shape \"answer\" + scip_index) carry real inputs."
                    .to_string(),
            );
            (ScoringConvention::admissible_edit(), true)
        }
        TaskShape::Answer => {
            notes.push(
                "answer-task convention inputs (trace_locality, trace_reach_depth) computed per \
                 pair from both arms' trial traces, the task oracle chain, and the declared \
                 scip_index; every admitted pair carries real inputs and pairs lacking them were \
                 excluded with reason (pre-registered 2026-07-04, aoa-dhk.1; see \
                 docs/r0_runbook.md)."
                    .to_string(),
            );
            (ScoringConvention::admissible_answer(), false)
        }
    };

    let config = FalsifyConfig {
        k_runs: manifest.k_runs,
        min_holdout_size: manifest.min_holdout_size,
        min_effect_size: manifest.min_effect_size,
        conventions,
    };
    let input = FalsifyInput { repos, config };
    let mut report = BuildReport {
        out_path: String::new(), // filled by the caller once the path is known
        observations_path: String::new(),
        observations_sha256: String::new(),
        observation_count: observations.len(),
        observation_ids: Vec::new(),
        repo_count: repo_builds.len(),
        total_identical_pairs,
        task_shape: shape,
        convention_inputs_degraded,
        repos: repo_builds,
        dropped_repos,
        notes,
    };
    observations.sort_by(|left, right| {
        (&left.repo_id, left.seed, &left.original_task_id, left.arm).cmp(&(
            &right.repo_id,
            right.seed,
            &right.original_task_id,
            right.arm,
        ))
    });
    report.observation_ids = observations
        .iter()
        .map(|observation| observation.id.to_string())
        .collect();
    Ok((input, report, observations))
}

/// Assemble a falsification input, build report, and content-addressed
/// observation sidecar records from one manifest.
pub fn build(
    manifest: &Manifest,
    tasks_dir: &Path,
    base_dir: &Path,
) -> crate::Result<(FalsifyInput, BuildReport, Vec<MeasurementObservationV1>)> {
    build_inner(manifest, tasks_dir, base_dir).map_err(FalsifyBuildError::from_anyhow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusion_evidence_preserves_raw_external_error_text() {
        let dir = tempfile::tempdir().unwrap();
        let trial = dir.path().join("task-1");
        std::fs::create_dir_all(&trial).unwrap();
        std::fs::write(
            trial.join("scoring.json"),
            r#"{"scorer_family":"dual_composite","error_direct":"boom\u001b[31mRED"}"#,
        )
        .unwrap();

        let outcomes = read_arm(dir.path()).unwrap();
        let reason = outcomes.excluded.get("task-1").unwrap().clone();

        assert!(reason.contains("boom\u{1b}[31mRED"));
        let json = serde_json::to_value(ExcludedTask {
            task_id: "task-1".to_string(),
            reason,
        })
        .unwrap();
        assert!(json["reason"].as_str().unwrap().contains('\u{1b}'));
    }
}
