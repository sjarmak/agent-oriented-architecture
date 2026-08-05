use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use aoa_gap::{ExposureStatus, SubjectKey};
use serde::{Deserialize, Serialize};

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
    /// Top-level `scoring.json` score values and their occurrence counts.
    pub score_distribution: BTreeMap<String, usize>,
    /// Trials with exposure evidence but no numeric top-level score.
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
    score_distribution: BTreeMap<String, usize>,
    unscored_trials: usize,
}

struct ExposureEvidence {
    spent_subjects: BTreeSet<SubjectKey>,
    provenance_by_repo: BTreeMap<String, ExposureProvenance>,
}

#[derive(Deserialize)]
struct ExposureScoring {
    score: Option<serde_json::Number>,
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
    let score = read_exposure_score(&trial_dir.join("scoring.json"))?;
    let run_path = trial_dir
        .parent()
        .expect("a discovered trial directory always has a parent")
        .to_path_buf();
    provenance
        .entry(repo_id.to_string())
        .or_default()
        .record(run_path, &mtimes, score);
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

fn read_exposure_score(path: &Path) -> Result<Option<String>, BenchError> {
    if !is_regular_file(path) {
        return Ok(None);
    }
    let scoring: ExposureScoring = read_json(path)?;
    Ok(scoring.score.map(|score| score.to_string()))
}

impl ProvenanceAccumulator {
    fn record(&mut self, run_path: PathBuf, mtimes: &[u64], score: Option<String>) {
        self.causing_run_paths.insert(run_path);
        self.earliest_unix_ms = mtimes.iter().copied().chain(self.earliest_unix_ms).min();
        self.latest_unix_ms = mtimes.iter().copied().chain(self.latest_unix_ms).max();
        self.trial_count += 1;
        if let Some(score) = score {
            *self.score_distribution.entry(score).or_default() += 1;
        } else {
            self.unscored_trials += 1;
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
            score_distribution: self.score_distribution,
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
