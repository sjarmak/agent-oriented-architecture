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

mod answer;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use answer::AnswerContext;
use aoa_bench::{
    aggregate_provenance, discover_tasks, load_task, scoring_path, transcript_path,
    AnswerMetricsV1, ArmIdentity, ArtifactDigestSetV1, CalibrationConclusion,
    CalibrationEvidenceV1, ExclusionReasonV1, GitObjectId, MeasurementMetricsV1,
    MeasurementObservationV1, MeasurementStateV1, MetricValueV1, Sha256Digest, TrialScoring,
    TRACE_LOCALITY_DEFINITION_VERSION, TRACE_REACH_DEPTH_DEFINITION_VERSION,
};
use aoa_falsify::{
    is_eligible, ConventionInputs, Eligibility, FalsifyConfig, FalsifyInput, PairTask, RepoResult,
    RepoRun, ScoringConvention,
};
use aoa_gap::HeldOutProvenance;
use aoa_metrics::Confidence;

use crate::cli::ExperimentArgs;
use crate::commands::fsutil::load_json_capped;
use crate::output::{escape_terminal, print_human, print_json};

/// Per-artifact and aggregate task-evidence read bound.
const MAX_EVIDENCE_BYTES: u64 = 128 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Manifest (operator-authored)
// ---------------------------------------------------------------------------

/// The whole build manifest. `deny_unknown_fields` on every operator-authored
/// boundary: a misspelled key (`min_effect_szie`) must fail loud, not silently
/// leave the real field at its default (0.0 disables the effect-size floor).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct RepoManifest {
    repo_id: String,
    /// Exact source revision whose task and graph evidence is being measured.
    repo_commit: GitObjectId,
    /// Operator assertion that the repo carries a SCIP-grade (high-confidence)
    /// index. REQUIRED — there is no safe default toward eligibility.
    confidence: ConfidenceDecl,
    /// Typed, content-addressed evidence backing calibration eligibility.
    calibration_artifact: PathBuf,
    /// Exact configuration bytes used by every repo-arm replication.
    repo_arm_config: PathBuf,
    /// Exact configuration bytes used by every harness-arm replication.
    harness_arm_config: PathBuf,
    /// The task shape this repo's trials carry. `answer` (comprehension tasks)
    /// computes real trace-locality/trace-reach convention inputs and REQUIRES
    /// `scip_index`; `edit` (the default) emits excluded observations until an
    /// edit-task metric pipeline exists.
    #[serde(default)]
    task_shape: TaskShape,
    /// Vendored SCIP JSON index for this repo (the `aoa eval run --scip-index`
    /// form), resolved relative to the manifest. Required for `answer` shape;
    /// rejected otherwise (it would silently do nothing).
    ///
    /// Pinned to the BASELINE-arm (pre-migration) repo state: one shared
    /// universe measures both arms symmetrically, and migration-added files are
    /// intentionally out-of-universe for both arms — see `docs/r0_runbook.md`
    /// § "Which SCIP index (pinned)".
    #[serde(default)]
    scip_index: Option<PathBuf>,
    runs: Vec<RunManifest>,
}

/// Declared task shape of a repo's trials. Spelled lowercase in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TaskShape {
    #[default]
    Edit,
    Answer,
}

/// One fixed-seed replication: the two arm run dirs over the same mined tasks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
}

/// The build report. `convention_inputs_degraded` is the load-bearing flag
/// `aoa falsify --build-meta` reads to decide whether to abstain.
#[derive(Debug, Serialize)]
pub(crate) struct BuildReport {
    out_path: String,
    observations_path: String,
    observations_sha256: String,
    observation_count: usize,
    observation_ids: Vec<String>,
    repo_count: usize,
    total_identical_pairs: usize,
    /// The (uniform) task shape of the manifest's repos, as data.
    task_shape: TaskShape,
    convention_inputs_degraded: bool,
    repos: Vec<RepoBuild>,
    /// Repos that contributed no identical pairs and were dropped from the
    /// input — kept here so their per-task exclusion reasons stay inspectable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dropped_repos: Vec<DroppedRepo>,
    notes: Vec<String>,
}

/// A repo dropped from the input (no identical pairs in some run), with the
/// per-task exclusion reasons that explain the drop. Eligibility facts are
/// deliberately absent: they are meaningless for a repo that supplies no
/// evidence.
#[derive(Debug, Serialize)]
struct DroppedRepo {
    repo_id: String,
    excluded_tasks: Vec<ExcludedTask>,
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

struct LoadedArtifact<T> {
    digest: Option<Sha256Digest>,
    value: Option<T>,
    reason: Option<ExclusionReasonV1>,
    diagnostic: Option<String>,
}

fn read_artifact(path: &Path) -> LoadedArtifact<Vec<u8>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMalformed),
                diagnostic: Some(format!("{} is not a regular file", path.display())),
            };
        }
        Err(error) => {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMissing),
                diagnostic: Some(format!("cannot read {}: {error}", path.display())),
            };
        }
    };
    if metadata.len() > MAX_EVIDENCE_BYTES {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMalformed),
            diagnostic: Some(format!(
                "{} exceeds {MAX_EVIDENCE_BYTES} byte evidence cap",
                path.display()
            )),
        };
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMissing),
                diagnostic: Some(format!("cannot open {}: {error}", path.display())),
            };
        }
    };
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if let Err(error) = std::io::Read::by_ref(&mut file)
        .take(MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMalformed),
            diagnostic: Some(format!("cannot read {}: {error}", path.display())),
        };
    }
    if bytes.len() as u64 > MAX_EVIDENCE_BYTES {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMalformed),
            diagnostic: Some(format!(
                "{} exceeds {MAX_EVIDENCE_BYTES} byte evidence cap",
                path.display()
            )),
        };
    }
    LoadedArtifact {
        digest: Some(Sha256Digest::of_bytes(&bytes)),
        value: Some(bytes),
        reason: None,
        diagnostic: None,
    }
}

fn read_calibration(path: &Path) -> LoadedArtifact<CalibrationEvidenceV1> {
    let raw = read_artifact(path);
    let Some(bytes) = raw.value else {
        return LoadedArtifact {
            digest: raw.digest,
            value: None,
            reason: Some(match raw.reason {
                Some(ExclusionReasonV1::ArtifactMissing) => ExclusionReasonV1::CalibrationMissing,
                _ => ExclusionReasonV1::CalibrationMalformed,
            }),
            diagnostic: raw.diagnostic,
        };
    };
    match serde_json::from_slice::<CalibrationEvidenceV1>(&bytes) {
        Ok(value) => match value.validate() {
            Ok(()) => LoadedArtifact {
                digest: raw.digest,
                value: Some(value),
                reason: None,
                diagnostic: None,
            },
            Err(error) => LoadedArtifact {
                digest: raw.digest,
                value: None,
                reason: Some(ExclusionReasonV1::CalibrationMalformed),
                diagnostic: Some(error.to_string()),
            },
        },
        Err(error) => LoadedArtifact {
            digest: raw.digest,
            value: None,
            reason: Some(ExclusionReasonV1::CalibrationMalformed),
            diagnostic: Some(format!(
                "invalid calibration artifact {}: {error}",
                path.display()
            )),
        },
    }
}

/// Digest every regular file below a task directory using a deterministic
/// filename-length/content-length framing. This covers every oracle input the
/// bench loader may consume without making relocation part of identity.
fn task_artifact_digest(task_dir: &Path) -> LoadedArtifact<()> {
    fn visit(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(&entry.path(), files)?;
            } else if file_type.is_file() {
                files.push(entry.path());
            } else if file_type.is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "symlinked task artifact refused: {}",
                        entry.path().display()
                    ),
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    if let Err(error) = visit(task_dir, &mut files) {
        return LoadedArtifact {
            digest: None,
            value: None,
            reason: Some(ExclusionReasonV1::ArtifactMissing),
            diagnostic: Some(format!(
                "cannot enumerate task artifacts under {}: {error}",
                task_dir.display()
            )),
        };
    }
    files.sort();
    let mut framed = Vec::new();
    for path in files {
        let relative = path.strip_prefix(task_dir).expect("visited below root");
        let loaded = read_artifact(&path);
        let Some(bytes) = loaded.value else {
            return LoadedArtifact {
                digest: loaded.digest,
                value: None,
                reason: loaded.reason,
                diagnostic: loaded.diagnostic,
            };
        };
        let Some(name) = relative.to_str() else {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMalformed),
                diagnostic: Some(format!(
                    "task artifact path {:?} is not valid UTF-8",
                    relative.as_os_str()
                )),
            };
        };
        if framed
            .len()
            .saturating_add(name.len())
            .saturating_add(bytes.len())
            .saturating_add(16)
            > MAX_EVIDENCE_BYTES as usize
        {
            return LoadedArtifact {
                digest: None,
                value: None,
                reason: Some(ExclusionReasonV1::ArtifactMalformed),
                diagnostic: Some(format!(
                    "task artifact bundle under {} exceeds {MAX_EVIDENCE_BYTES} byte cap",
                    task_dir.display()
                )),
            };
        }
        framed.extend_from_slice(&(name.len() as u64).to_be_bytes());
        framed.extend_from_slice(name.as_bytes());
        framed.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        framed.extend_from_slice(&bytes);
    }
    LoadedArtifact {
        digest: Some(Sha256Digest::of_bytes(&framed)),
        value: Some(()),
        reason: None,
        diagnostic: None,
    }
}

struct RepoEvidence {
    calibration: LoadedArtifact<CalibrationEvidenceV1>,
    repo_config: LoadedArtifact<Vec<u8>>,
    harness_config: LoadedArtifact<Vec<u8>>,
    index: Option<LoadedArtifact<Vec<u8>>>,
}

fn first_evidence_problem(
    values: &[(&LoadedArtifact<Vec<u8>>, ExclusionReasonV1)],
) -> Option<(ExclusionReasonV1, String)> {
    values.iter().find_map(|(loaded, fallback)| {
        loaded.value.is_none().then(|| {
            (
                loaded.reason.unwrap_or(*fallback),
                loaded
                    .diagnostic
                    .clone()
                    .unwrap_or_else(|| "required evidence unavailable".to_string()),
            )
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn build_observation(
    repo: &RepoManifest,
    seed: u64,
    arm: ArmIdentity,
    run_dir: &Path,
    task_id: &str,
    outcome: Option<Result<bool, String>>,
    tasks_dir: &Path,
    repo_evidence: &RepoEvidence,
    answer_ctx: Option<&mut AnswerContext>,
    answer_context_error: Option<&str>,
) -> Result<(MeasurementObservationV1, Option<String>)> {
    let scoring = read_artifact(&scoring_path(run_dir, task_id));
    let trace = read_artifact(&transcript_path(run_dir, task_id));
    let oracle = task_artifact_digest(&tasks_dir.join(task_id));
    let config = match arm {
        ArmIdentity::Repo => &repo_evidence.repo_config,
        ArmIdentity::Harness => &repo_evidence.harness_config,
    };
    let evidence = ArtifactDigestSetV1 {
        scoring: scoring.digest.clone(),
        oracle: oracle.digest.clone(),
        trace: trace.digest.clone(),
        index: repo_evidence
            .index
            .as_ref()
            .and_then(|loaded| loaded.digest.clone()),
        config: config.digest.clone(),
        calibration: repo_evidence.calibration.digest.clone(),
    };

    let excluded =
        |reason, diagnostic: String| (MeasurementStateV1::Excluded { reason }, diagnostic);
    let (state, diagnostic) = match outcome {
        None => excluded(
            ExclusionReasonV1::ArmArtifactMissing,
            format!("{arm:?} arm has no trial artifacts for task {task_id}"),
        ),
        Some(Err(diagnostic)) => excluded(ExclusionReasonV1::ArtifactMalformed, diagnostic),
        Some(Ok(held_out_success)) => {
            let required = [
                (&scoring, ExclusionReasonV1::ArtifactMissing),
                (&trace, ExclusionReasonV1::ArtifactMissing),
                (config, ExclusionReasonV1::ArtifactMissing),
            ];
            if let Some((reason, diagnostic)) = first_evidence_problem(&required) {
                excluded(reason, diagnostic)
            } else if oracle.value.is_none() {
                excluded(
                    oracle.reason.unwrap_or(ExclusionReasonV1::ArtifactMissing),
                    oracle
                        .diagnostic
                        .clone()
                        .unwrap_or_else(|| "oracle evidence unavailable".to_string()),
                )
            } else if let Some(reason) = repo_evidence.calibration.reason {
                excluded(
                    reason,
                    repo_evidence
                        .calibration
                        .diagnostic
                        .clone()
                        .unwrap_or_else(|| "calibration evidence unavailable".to_string()),
                )
            } else {
                let calibration = repo_evidence
                    .calibration
                    .value
                    .clone()
                    .expect("reason-free calibration has a value");
                if calibration.conclusion != CalibrationConclusion::Calibrated {
                    excluded(
                        ExclusionReasonV1::CalibrationNotEstablished,
                        "calibration artifact conclusion is not calibrated".to_string(),
                    )
                } else if repo.task_shape == TaskShape::Edit {
                    excluded(
                        ExclusionReasonV1::MetricEvidenceMissing,
                        "edit-shaped observation requires retrieval, invariant, mutation, and edit-locality evidence; degraded sentinels are forbidden".to_string(),
                    )
                } else if let Some(error) = answer_context_error {
                    excluded(ExclusionReasonV1::MetricEvidenceMissing, error.to_string())
                } else {
                    let Some(ctx) = answer_ctx else {
                        unreachable!("answer context or its error is present")
                    };
                    match ctx.observation_inputs(
                        task_id,
                        run_dir,
                        match arm {
                            ArmIdentity::Repo => "repo",
                            ArmIdentity::Harness => "harness",
                        },
                    ) {
                        Ok((trace_locality, trace_reach_depth)) => {
                            let task = load_task(tasks_dir.join(task_id)).with_context(|| {
                                format!("failed to load task {task_id} provenance")
                            })?;
                            (
                                MeasurementStateV1::Measured {
                                    held_out_success,
                                    held_out_provenance: task.held_out_provenance(),
                                    calibration,
                                    metrics: Box::new(MeasurementMetricsV1::Answer(
                                        AnswerMetricsV1 {
                                            trace_locality: MetricValueV1::new(
                                                TRACE_LOCALITY_DEFINITION_VERSION,
                                                trace_locality,
                                            ),
                                            trace_reach_depth: MetricValueV1::new(
                                                TRACE_REACH_DEPTH_DEFINITION_VERSION,
                                                trace_reach_depth,
                                            ),
                                        },
                                    )),
                                },
                                String::new(),
                            )
                        }
                        Err(error) => excluded(
                            ExclusionReasonV1::MetricComputationFailed,
                            format!("{error:#}"),
                        ),
                    }
                }
            }
        }
    };
    let observation = MeasurementObservationV1::new(
        repo.repo_id.clone(),
        repo.repo_commit.clone(),
        task_id.to_string(),
        seed,
        arm,
        evidence,
        state,
    )?;
    Ok((observation, (!diagnostic.is_empty()).then_some(diagnostic)))
}

/// Assemble one repo's `RepoResult` over its fixed-seed runs, collecting the
/// per-repo build provenance. `answer_ctx` is present exactly for answer-shaped
/// repos and computes each pair's real convention inputs.
fn build_repo(
    repo: &RepoManifest,
    tasks_dir: &Path,
    base_dir: &Path,
    k_runs: u32,
    mut answer_ctx: Option<AnswerContext>,
    answer_context_error: Option<String>,
) -> Result<RepoOutcome> {
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
        return Ok(RepoOutcome::Dropped(
            DroppedRepo {
                repo_id: repo.repo_id.clone(),
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
        calibrated,
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
fn validated_shape(manifest: &Manifest) -> Result<TaskShape> {
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
fn build(
    manifest: &Manifest,
    tasks_dir: &Path,
    base_dir: &Path,
) -> Result<(FalsifyInput, BuildReport, Vec<MeasurementObservationV1>)> {
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

/// Path the build report is written to: the `--out` path with a `.build.json`
/// extension (e.g. `falsify_input.json` -> `falsify_input.build.json`).
fn build_report_path(out: &Path) -> PathBuf {
    out.with_extension("build.json")
}

fn observations_path(out: &Path) -> PathBuf {
    out.with_extension("observations.jsonl")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("output path must have a UTF-8 file name")?;
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("failed to install {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Run `aoa eval experiment`.
pub(crate) fn run(args: &ExperimentArgs) -> Result<i32> {
    let manifest: Manifest = load_json_capped(&args.manifest, "manifest")?;

    let base_dir = args.manifest.parent().unwrap_or_else(|| Path::new("."));
    let (input, mut report, observations) = build(&manifest, &args.tasks, base_dir)?;

    let input_json = serde_json::to_string_pretty(&input)?;

    let mut observations_jsonl = Vec::new();
    for observation in &observations {
        serde_json::to_writer(&mut observations_jsonl, observation)?;
        observations_jsonl.push(b'\n');
    }
    let observation_path = observations_path(&args.out);
    report.out_path = args.out.display().to_string();
    report.observations_path = observation_path.display().to_string();
    report.observations_sha256 = Sha256Digest::of_bytes(&observations_jsonl).to_string();
    let report_path = build_report_path(&args.out);
    let report_json = serde_json::to_string_pretty(&report)?;
    write_atomic(&args.out, input_json.as_bytes())?;
    write_atomic(&observation_path, &observations_jsonl)?;
    write_atomic(&report_path, format!("{report_json}\n").as_bytes())?;

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
            escape_terminal(&r.repo_id),
            r.identical_pairs,
            r.holdout_size,
            r.native_span,
            r.confidence,
            r.calibrated,
            r.eligible,
        );
        for ex in &r.excluded_tasks {
            let _ = writeln!(
                out,
                "      excluded {}: {}",
                escape_terminal(&ex.task_id),
                escape_terminal(&ex.reason)
            );
        }
    }
    for d in &report.dropped_repos {
        let _ = writeln!(
            out,
            "  {:<24} DROPPED: no identical pairs",
            escape_terminal(&d.repo_id)
        );
        for ex in &d.excluded_tasks {
            let _ = writeln!(
                out,
                "      excluded {}: {}",
                escape_terminal(&ex.task_id),
                escape_terminal(&ex.reason)
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
    use crate::commands::fsutil::MAX_JSON_BYTES;

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
    fn exclusion_evidence_preserves_raw_external_error_text() {
        let dir = std::env::temp_dir().join(format!(
            "aoa-experiment-raw-evidence-{}",
            std::process::id()
        ));
        let trial = dir.join("task-1");
        std::fs::create_dir_all(&trial).unwrap();
        std::fs::write(
            trial.join("scoring.json"),
            r#"{"scorer_family":"dual_composite","error_direct":"boom\u001b[31mRED"}"#,
        )
        .unwrap();

        let outcomes = read_arm(&dir).unwrap();
        let reason = outcomes.excluded.get("task-1").unwrap().clone();
        assert!(reason.contains("boom\u{1b}[31mRED"));

        let json = serde_json::to_value(ExcludedTask {
            task_id: "task-1".to_string(),
            reason,
        })
        .unwrap();
        assert!(json["reason"].as_str().unwrap().contains('\u{1b}'));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn human_report_escapes_external_exclusion_diagnostics() {
        let report = BuildReport {
            out_path: "out.json".to_string(),
            observations_path: "out.observations.jsonl".to_string(),
            observations_sha256: "a".repeat(64),
            observation_count: 1,
            observation_ids: vec!["b".repeat(64)],
            repo_count: 0,
            total_identical_pairs: 0,
            task_shape: TaskShape::Answer,
            convention_inputs_degraded: false,
            repos: Vec::new(),
            dropped_repos: vec![DroppedRepo {
                repo_id: "repo".to_string(),
                excluded_tasks: vec![ExcludedTask {
                    task_id: "task".to_string(),
                    reason: "boom\u{1b}[31mRED".to_string(),
                }],
            }],
            notes: Vec::new(),
        };
        let rendered = render_human(&report, Path::new("out.build.json"));
        assert!(!rendered.contains('\u{1b}'));
        assert!(rendered.contains(r"\u{1b}"));
    }

    #[cfg(unix)]
    #[test]
    fn evidence_reader_refuses_a_symlink_without_hashing_its_target() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!(
            "aoa-experiment-symlink-evidence-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.json");
        let planted = dir.join("config.json");
        std::fs::write(&victim, br#"{"secret":"outside"}"#).unwrap();
        symlink(&victim, &planted).unwrap();

        let loaded = read_artifact(&planted);
        assert!(loaded.value.is_none());
        assert!(loaded.digest.is_none());
        assert_eq!(loaded.reason, Some(ExclusionReasonV1::ArtifactMalformed));
        std::fs::remove_dir_all(&dir).ok();
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
