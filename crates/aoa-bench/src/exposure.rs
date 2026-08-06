use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use aoa_gap::{ExposureStatus, SubjectKey};
use serde::{Deserialize, Serialize};

use crate::codeprobe_run::TrialScoring;
use crate::error::BenchError;
use crate::loader::{read_capped, MAX_CODEPROBE_JSON_BYTES};

const PREP_FILE: &str = "prep.json";
const MINE_FILE: &str = "mine.json";
const RESOLVED_INSTRUCTION: &str = "instruction.resolved.md";
const MAX_EXPOSURE_ENTRIES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Exposure results for every admitted repository found below a campaign root.
pub struct ExposureScan {
    /// Deterministically ordered by repository id.
    pub repos: Vec<RepoExposure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One pinned repository corpus and its derived exposure status.
pub struct RepoExposure {
    /// Stable campaign repository identifier.
    pub repo_id: String,
    /// Exact baseline revision read from `prep.json`.
    pub baseline_commit: String,
    /// Number of distinct admitted subject keys in the current corpus.
    pub total_subjects: usize,
    /// Exposure derived from all persisted trial artifacts.
    pub status: ExposureStatus,
    /// Artifact provenance for admitted subjects that contributed to `status`.
    /// Absence is meaningful: an unexposed repository has no causing run.
    pub provenance: Option<ExposureProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Persisted trial evidence that caused a repository's exposure verdict.
pub struct ExposureProvenance {
    /// Run roots containing the admitted subjects' persisted trial artifacts.
    pub causing_run_paths: BTreeSet<PathBuf>,
    /// Earliest and latest modification times across qualifying evidence files.
    pub mtime_range: ExposureMtimeRange,
    /// Number of distinct trial directories contributing to the verdict.
    pub trial_count: usize,
    /// Contributing trials whose held-out outcome was a pass.
    ///
    /// Held-out means what [`TrialScoring::held_out_outcome`] means: a
    /// `dual_composite` trial contributes its independent artifact leg, never
    /// its top-level composite, which a failed direct leg can lower.
    pub held_out_passed: usize,
    /// Contributing trials whose held-out outcome was a fail.
    pub held_out_failed: usize,
    /// Trials whose `scoring.json` persisted a scorer error — top level or on
    /// either dual leg — so the held-out outcome is unknown rather than absent.
    ///
    /// Its own count, not folded into `unscored_trials`: a scorer that crashed
    /// is evidence the subject was spent *and* that the ledger cannot say how
    /// the trial went. It does not abort the scan — exposure asks whether a
    /// subject was touched, and a run whose scorer failed touched it.
    pub errored_trials: usize,
    /// Trials with exposure evidence but no held-out signal at all — no
    /// `scoring.json`, or one carrying neither a `passed` flag nor a score.
    pub unscored_trials: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Evidence-file modification interval expressed without timezone ambiguity.
pub struct ExposureMtimeRange {
    pub earliest_unix_ms: u64,
    pub latest_unix_ms: u64,
}

impl RepoExposure {
    /// Number of admitted subjects with at least one persisted trial artifact.
    pub fn exposed_subject_count(&self) -> usize {
        match &self.status {
            ExposureStatus::Unexposed => 0,
            ExposureStatus::PartiallyExposed { subjects } => subjects.len(),
            ExposureStatus::Exposed => self.total_subjects,
        }
    }
}

/// Codeprobe writes `prep.json` and `mine.json`, and adds keys to them over
/// time. Being declared subsets of a producer-owned shape rather than
/// operator-authored input, both must tolerate unknown fields:
/// `deny_unknown_fields` would reject files this scanner has no quarrel with.
/// The same rationale [`TrialScoring`] records at its own declaration.
#[derive(Debug, Deserialize)]
struct PrepManifest {
    repo: String,
    baseline_path: PathBuf,
    baseline_sha: String,
}

#[derive(Debug, Deserialize)]
struct MineManifest {
    repo: String,
    task_ids: Vec<String>,
}

struct Corpus {
    repo_id: String,
    baseline_commit: String,
    subjects: BTreeSet<SubjectKey>,
}

#[derive(Default)]
struct ProvenanceAccumulator {
    causing_run_paths: BTreeSet<PathBuf>,
    earliest_unix_ms: Option<u64>,
    latest_unix_ms: Option<u64>,
    trial_count: usize,
    held_out_passed: usize,
    held_out_failed: usize,
    errored_trials: usize,
    unscored_trials: usize,
}

struct ExposureEvidence {
    spent_subjects: BTreeSet<SubjectKey>,
    provenance_by_repo: BTreeMap<String, ExposureProvenance>,
}

/// Scan every codeprobe trial under `runs_root`, including quarantine trees,
/// and compare the spent subjects with each campaign repo's admitted corpus.
pub fn scan_exposure(runs_root: &Path) -> Result<ExposureScan, BenchError> {
    let files = walk_regular_files(runs_root)?;
    let corpora = load_corpora(&files)?;
    if corpora.is_empty() {
        return Err(BenchError::NoExposureCorpora(runs_root.to_path_buf()));
    }
    let evidence = load_exposure_evidence(&files, &corpora)?;
    Ok(ExposureScan {
        repos: classify(corpora, &evidence),
    })
}

fn walk_regular_files(root: &Path) -> Result<Vec<PathBuf>, BenchError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut visited = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| BenchError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| BenchError::Io {
                path: dir.clone(),
                source,
            })?;
            visited += 1;
            if visited > MAX_EXPOSURE_ENTRIES {
                return Err(BenchError::TooManyExposureEntries {
                    root: root.to_path_buf(),
                    max: MAX_EXPOSURE_ENTRIES,
                });
            }
            let kind = entry.file_type().map_err(|source| BenchError::Io {
                path: entry.path(),
                source,
            })?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn load_corpora(files: &[PathBuf]) -> Result<BTreeMap<String, Corpus>, BenchError> {
    for mine_path in files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == MINE_FILE))
    {
        let dir = mine_path.parent().expect("a file always has a parent");
        if !is_regular_file(&dir.join(PREP_FILE)) {
            return Err(BenchError::MissingExposurePrep(mine_path.clone()));
        }
    }
    let mut corpora = BTreeMap::new();
    for prep_path in files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == PREP_FILE))
    {
        let dir = prep_path.parent().expect("a file always has a parent");
        let mine_path = dir.join(MINE_FILE);
        if !is_regular_file(&mine_path) {
            return Err(BenchError::MissingExposureMine((*prep_path).clone()));
        }
        let corpus = load_corpus(prep_path, &mine_path)?;
        if corpora.contains_key(&corpus.repo_id) {
            return Err(BenchError::DuplicateExposureRepo {
                repo_id: corpus.repo_id,
            });
        }
        corpora.insert(corpus.repo_id.clone(), corpus);
    }
    Ok(corpora)
}

fn load_corpus(prep_path: &Path, mine_path: &Path) -> Result<Corpus, BenchError> {
    let prep: PrepManifest = read_json(prep_path)?;
    let mine: MineManifest = read_json(mine_path)?;
    if prep.repo != mine.repo {
        return Err(BenchError::ExposureManifestMismatch {
            dir: prep_path.parent().unwrap().to_path_buf(),
            prep_repo: prep.repo,
            mine_repo: mine.repo,
        });
    }
    if mine.task_ids.is_empty() {
        return Err(BenchError::EmptyExposureCorpus { repo_id: prep.repo });
    }
    let mut subjects = BTreeSet::new();
    for task_id in mine.task_ids {
        validate_task_id(&task_id, mine_path)?;
        let path = prep
            .baseline_path
            .join(".codeprobe/tasks")
            .join(task_id)
            .join("instruction.md");
        let raw = read_capped(&path, MAX_CODEPROBE_JSON_BYTES)?;
        subjects.insert(parse_subject(&raw, &prep.repo, &prep.baseline_sha, &path)?);
    }
    Ok(Corpus {
        repo_id: prep.repo,
        baseline_commit: prep.baseline_sha,
        subjects,
    })
}

fn validate_task_id(task_id: &str, mine_path: &Path) -> Result<(), BenchError> {
    let mut components = Path::new(task_id).components();
    let valid = matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    );
    if !valid {
        return Err(BenchError::InvalidExposureTaskId {
            mine_path: mine_path.to_path_buf(),
            task_id: task_id.to_string(),
        });
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, BenchError> {
    let raw = read_capped(path, MAX_CODEPROBE_JSON_BYTES)?;
    serde_json::from_str(&raw).map_err(|source| BenchError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn load_exposure_evidence(
    files: &[PathBuf],
    corpora: &BTreeMap<String, Corpus>,
) -> Result<ExposureEvidence, BenchError> {
    let trial_dirs: BTreeSet<_> = files
        .iter()
        .filter(|path| is_evidence_file(path))
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    let mut spent = BTreeSet::new();
    let mut provenance = BTreeMap::<String, ProvenanceAccumulator>::new();
    for trial_dir in trial_dirs {
        let instruction_path = trial_dir.join(RESOLVED_INSTRUCTION);
        if !is_regular_file(&instruction_path) {
            return Err(identity_error(
                &trial_dir,
                "missing instruction.resolved.md",
            ));
        }
        let raw = read_capped(&instruction_path, MAX_CODEPROBE_JSON_BYTES)?;
        let repo_id = parse_repo(&raw)
            .ok_or_else(|| identity_error(&instruction_path, "missing **Repository:** header"))?;
        let corpus = corpora.get(repo_id).ok_or_else(|| {
            identity_error(
                &instruction_path,
                format!("repository {repo_id:?} has no prep.json + mine.json corpus"),
            )
        })?;
        let subject = parse_subject(&raw, repo_id, &corpus.baseline_commit, &instruction_path)?;
        if corpus.subjects.contains(&subject) {
            spent.insert(subject);
            record_trial_provenance(&trial_dir, repo_id, &mut provenance)?;
        }
    }
    let provenance_by_repo = provenance
        .into_iter()
        .map(|(repo_id, accumulator)| Ok((repo_id, accumulator.finish()?)))
        .collect::<Result<_, BenchError>>()?;
    Ok(ExposureEvidence {
        spent_subjects: spent,
        provenance_by_repo,
    })
}

fn classify(corpora: BTreeMap<String, Corpus>, evidence: &ExposureEvidence) -> Vec<RepoExposure> {
    corpora
        .into_values()
        .map(|corpus| {
            let exposed: BTreeSet<_> = corpus
                .subjects
                .intersection(&evidence.spent_subjects)
                .cloned()
                .collect();
            let status = match exposed.len() {
                0 => ExposureStatus::Unexposed,
                n if n == corpus.subjects.len() => ExposureStatus::Exposed,
                _ => ExposureStatus::PartiallyExposed { subjects: exposed },
            };
            RepoExposure {
                provenance: evidence.provenance_by_repo.get(&corpus.repo_id).cloned(),
                repo_id: corpus.repo_id,
                baseline_commit: corpus.baseline_commit,
                total_subjects: corpus.subjects.len(),
                status,
            }
        })
        .collect()
}

fn record_trial_provenance(
    trial_dir: &Path,
    repo_id: &str,
    provenance: &mut BTreeMap<String, ProvenanceAccumulator>,
) -> Result<(), BenchError> {
    let evidence_paths: Vec<_> = ["scoring.json", "agent_output.txt"]
        .into_iter()
        .map(|name| trial_dir.join(name))
        .filter(|path| is_regular_file(path))
        .collect();
    let mtimes = evidence_paths
        .iter()
        .map(|path| modified_unix_ms(path))
        .collect::<Result<Vec<_>, _>>()?;
    let run_path = trial_dir
        .parent()
        .expect("a discovered trial directory always has a parent")
        .to_path_buf();
    let held_out = read_held_out_outcome(&run_path, trial_dir)?;
    provenance
        .entry(repo_id.to_string())
        .or_default()
        .record(run_path, &mtimes, held_out);
    Ok(())
}

fn modified_unix_ms(path: &Path) -> Result<u64, BenchError> {
    let modified = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| BenchError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|_| BenchError::ExposureMtimeBeforeEpoch(path.to_path_buf()))?;
    u64::try_from(duration.as_millis())
        .map_err(|_| BenchError::ExposureMtimeOverflow(path.to_path_buf()))
}

/// The ledger's held-out verdict for one trial, read through the crate's one
/// `scoring.json` declaration rather than a private copy of it.
///
/// A trial that persisted no `scoring.json` still counts as evidence of
/// exposure — the agent ran against the subject either way — so its absence
/// degrades to `Absent`.
///
/// A persisted scorer error is a third thing. [`TrialScoring::held_out_outcome`]
/// raises on one, which is right for a gate deciding a result but wrong here:
/// exposure asks whether a subject was spent, and a trial whose scorer crashed
/// spent it — aborting the whole scan on one bad trial answers a weaker
/// question than the one asked. So both persisted-error variants are caught and
/// tallied rather than propagated. They are matched off the owner's error type
/// rather than pre-checked field by field, because a `dual_composite` file can
/// carry a leg error (`error_artifact`) with no top-level `error` at all, and a
/// pre-check that only read the top level would still abort the scan on it.
fn read_held_out_outcome(run_dir: &Path, trial_dir: &Path) -> Result<HeldOutTally, BenchError> {
    if !is_regular_file(&trial_dir.join("scoring.json")) {
        return Ok(HeldOutTally::Absent);
    }
    let name = trial_dir
        .file_name()
        .expect("a discovered trial directory always has a file name");
    // Reject a non-UTF-8 trial name rather than lossily rendering it: the id is
    // rejoined onto `run_dir` to locate the file, and `to_string_lossy` is not
    // injective, so two distinct dirents could collapse onto one trial's
    // `scoring.json`. Same rule, and the same reason, as `discover_tasks`.
    let task_id = name.to_str().ok_or_else(|| BenchError::TrialNameNotUtf8 {
        run_dir: run_dir.to_path_buf(),
        name: name.to_os_string(),
    })?;
    match TrialScoring::load(run_dir, task_id)?.held_out_outcome() {
        Ok(Some(true)) => Ok(HeldOutTally::Passed),
        Ok(Some(false)) => Ok(HeldOutTally::Failed),
        Ok(None) => Ok(HeldOutTally::Absent),
        Err(BenchError::ScoringErrored { .. } | BenchError::ScoringLegErrored { .. }) => {
            Ok(HeldOutTally::Errored)
        }
        Err(other) => Err(other),
    }
}

/// How one trial's held-out outcome lands in the provenance report.
enum HeldOutTally {
    Passed,
    Failed,
    /// `scoring.json` persisted a scorer error, top-level or per-leg: the
    /// outcome is unknown rather than absent.
    Errored,
    /// No `scoring.json`, or one carrying no pass signal at all.
    Absent,
}

impl ProvenanceAccumulator {
    fn record(&mut self, run_path: PathBuf, mtimes: &[u64], held_out: HeldOutTally) {
        self.causing_run_paths.insert(run_path);
        self.earliest_unix_ms = mtimes.iter().copied().chain(self.earliest_unix_ms).min();
        self.latest_unix_ms = mtimes.iter().copied().chain(self.latest_unix_ms).max();
        self.trial_count += 1;
        match held_out {
            HeldOutTally::Passed => self.held_out_passed += 1,
            HeldOutTally::Failed => self.held_out_failed += 1,
            HeldOutTally::Errored => self.errored_trials += 1,
            HeldOutTally::Absent => self.unscored_trials += 1,
        }
    }

    fn finish(self) -> Result<ExposureProvenance, BenchError> {
        let earliest_unix_ms = self
            .earliest_unix_ms
            .ok_or(BenchError::EmptyExposureProvenance)?;
        let latest_unix_ms = self
            .latest_unix_ms
            .ok_or(BenchError::EmptyExposureProvenance)?;
        Ok(ExposureProvenance {
            causing_run_paths: self.causing_run_paths,
            mtime_range: ExposureMtimeRange {
                earliest_unix_ms,
                latest_unix_ms,
            },
            trial_count: self.trial_count,
            held_out_passed: self.held_out_passed,
            held_out_failed: self.held_out_failed,
            errored_trials: self.errored_trials,
            unscored_trials: self.unscored_trials,
        })
    }
}

fn parse_subject(
    instruction: &str,
    repo_id: &str,
    baseline_commit: &str,
    path: &Path,
) -> Result<SubjectKey, BenchError> {
    let heading = instruction
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .ok_or_else(|| identity_error(path, "missing '# <family>: <subject>' heading"))?;
    let (family, subject) = heading
        .split_once(": ")
        .filter(|(family, subject)| !family.trim().is_empty() && !subject.trim().is_empty())
        .ok_or_else(|| identity_error(path, "malformed '# <family>: <subject>' heading"))?;
    Ok(SubjectKey {
        repo_id: repo_id.to_string(),
        baseline_commit: baseline_commit.to_string(),
        oracle_target_symbol: subject.trim().to_string(),
        question_family: family.trim().to_string(),
    })
}

fn parse_repo(instruction: &str) -> Option<&str> {
    instruction.lines().find_map(|line| {
        line.strip_prefix("**Repository:** ")
            .map(str::trim)
            .filter(|repo| !repo.is_empty())
    })
}

fn is_evidence_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "scoring.json" || name == "agent_output.txt")
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn identity_error(path: &Path, message: impl Into<String>) -> BenchError {
    BenchError::ExposureIdentity {
        path: path.to_path_buf(),
        message: message.into(),
    }
}
