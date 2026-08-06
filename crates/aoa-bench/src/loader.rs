use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::BenchError;
use crate::oracle::OracleChainFacts;
use crate::task::{AcceptedSolution, CodeprobeTask};

/// Whether `dir` is a codeprobe task directory — one carrying either a
/// `metadata.json` (org-scale layout) or a `task.toml` (probe layout). This is
/// the single predicate [`load_task`] guards on and `aoa`'s corpus discovery
/// uses as its leaf test, so the two cannot drift on what counts as a task dir.
pub fn is_task_dir(dir: &Path) -> bool {
    dir.join("metadata.json").exists() || dir.join("task.toml").exists()
}

/// Load a codeprobe task directory into AOA task inputs.
///
/// Supports both observed codeprobe layouts: the rich org-scale form
/// (`metadata.json` carrying the oracle answer and ground-truth commit) and the
/// simple probe form (`task.toml` + `tests/ground_truth.json`). The oracle file
/// set is read from `ground_truth.json` (top-level or under `tests/`); the
/// per-backend accepted solutions are read from `divergence_report.json`,
/// codeprobe's record of what each consensus-mining backend independently found.
pub fn load_task(dir: impl AsRef<Path>) -> Result<CodeprobeTask, BenchError> {
    let dir = dir.as_ref();
    if !is_task_dir(dir) {
        return Err(BenchError::NotATask(dir.to_path_buf()));
    }

    // metadata.json takes precedence when both manifests are present; the
    // `is_task_dir` guard above guarantees at least one exists, so the `else`
    // reads `task.toml`.
    let metadata_path = dir.join("metadata.json");
    let manifest = if metadata_path.exists() {
        read_metadata(&metadata_path)?
    } else {
        read_toml(&dir.join("task.toml"))?
    };

    let instruction = read_instruction(dir)?;
    let gt = read_ground_truth(dir, &manifest)?;

    Ok(CodeprobeTask {
        id: manifest.id,
        repo: manifest.repo,
        instruction,
        oracle_files: gt.oracle_files,
        ground_truth_commit: manifest.ground_truth_commit.or(gt.commit),
        accepted_solutions: gt.accepted_solutions,
        oracle_chain: gt.oracle_chain,
    })
}

/// The manifest facts a loader needs, regardless of which layout supplied them.
struct Manifest {
    id: String,
    repo: String,
    ground_truth_commit: Option<String>,
    /// Oracle file list carried inline in the manifest (org-scale `metadata.json`).
    inline_oracle_files: Option<BTreeSet<String>>,
}

// ---------------------------------------------------------------------------
// metadata.json (org-scale layout)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MetadataFile {
    id: String,
    repo: String,
    #[serde(default)]
    metadata: MetadataBlock,
    #[serde(default)]
    verification: VerificationBlock,
}

#[derive(Deserialize, Default)]
struct MetadataBlock {
    #[serde(default)]
    ground_truth_commit: Option<String>,
}

#[derive(Deserialize, Default)]
struct VerificationBlock {
    #[serde(default)]
    oracle_answer: Vec<String>,
}

fn read_metadata(path: &Path) -> Result<Manifest, BenchError> {
    let raw = read_file(path)?;
    let file: MetadataFile = serde_json::from_str(&raw).map_err(|source| BenchError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let commit = meaningful_commit(file.metadata.ground_truth_commit);
    let inline = if file.verification.oracle_answer.is_empty() {
        None
    } else {
        Some(file.verification.oracle_answer.into_iter().collect())
    };
    Ok(Manifest {
        id: file.id,
        repo: file.repo,
        ground_truth_commit: commit,
        inline_oracle_files: inline,
    })
}

// ---------------------------------------------------------------------------
// task.toml (probe layout)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TomlFile {
    task: TomlTask,
}

#[derive(Deserialize)]
struct TomlTask {
    id: String,
    repo: String,
}

fn read_toml(path: &Path) -> Result<Manifest, BenchError> {
    let raw = read_file(path)?;
    let file: TomlFile = toml::from_str(&raw).map_err(|source| BenchError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Manifest {
        id: file.task.id,
        repo: file.task.repo,
        ground_truth_commit: None,
        inline_oracle_files: None,
    })
}

// ---------------------------------------------------------------------------
// Shared readers
// ---------------------------------------------------------------------------

fn read_instruction(dir: &Path) -> Result<String, BenchError> {
    let path = dir.join("instruction.md");
    Ok(read_file(&path)?.trim().to_string())
}

/// The file-list payload of a `ground_truth.json`.
#[derive(Deserialize)]
struct GroundTruthFile {
    #[serde(default)]
    expected: Vec<String>,
    /// History commit the answer was mined against. Present in the probe layout,
    /// where the org-scale `metadata.json` block is absent.
    #[serde(default)]
    commit: Option<String>,
    /// comprehension-v1 answer payload (file list, boolean, text or count).
    #[serde(default)]
    answer: serde_json::Value,
    /// comprehension-v1 answer type; `"file_list"` marks `answer` as file paths.
    #[serde(default)]
    answer_type: Option<String>,
}

impl GroundTruthFile {
    /// The comprehension-v1 answer files: `answer` entries when the recorded
    /// `answer_type` is `file_list`. Any other answer type carries no file
    /// references, and non-string entries are ignored rather than guessed at.
    fn answer_files(&self) -> BTreeSet<String> {
        if self.answer_type.as_deref() != Some("file_list") {
            return BTreeSet::new();
        }
        self.answer
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .map(str::to_owned)
            .collect()
    }
}

/// The oracle facts resolved from a task's ground-truth files.
struct GroundTruth {
    oracle_files: BTreeSet<String>,
    accepted_solutions: Vec<AcceptedSolution>,
    /// History commit recorded alongside the file-list oracle, if any.
    commit: Option<String>,
    /// Answer-task oracle-chain references (comprehension-v1 + consensus.v1).
    oracle_chain: OracleChainFacts,
}

/// Resolve the oracle file set, the per-backend accepted solutions, and the
/// mining commit.
///
/// The oracle file set (`G_t`) comes from the manifest's inline answer when
/// present, otherwise from `ground_truth.json` (the consensus answer). The
/// accepted solutions come from `divergence_report.json`, codeprobe's record of
/// what *each* backend independently mined — see [`read_accepted_backends`]. The
/// canonical file's own `commit`, if any, is returned so a probe-layout task can
/// be recognized as externally composed even without an org-scale metadata block.
fn read_ground_truth(dir: &Path, manifest: &Manifest) -> Result<GroundTruth, BenchError> {
    let gt_dir = ground_truth_dir(dir);

    let canonical = parse_ground_truth(&gt_dir.join("ground_truth.json"))?;
    let oracle_files = match (&manifest.inline_oracle_files, &canonical) {
        (Some(inline), _) => inline.clone(),
        (None, Some(gt)) => gt.expected.iter().cloned().collect(),
        (None, None) => BTreeSet::new(),
    };
    let commit = meaningful_commit(canonical.as_ref().and_then(|gt| gt.commit.clone()));

    let divergence = read_divergence(dir)?;
    let oracle_chain = OracleChainFacts {
        answer_files: canonical
            .as_ref()
            .map(GroundTruthFile::answer_files)
            .unwrap_or_default(),
        consensus_files: divergence.consensus_files,
        defining_file: divergence.defining_file,
        symbol: divergence.symbol,
    };

    Ok(GroundTruth {
        oracle_files,
        accepted_solutions: divergence.accepted_solutions,
        commit,
        oracle_chain,
    })
}

// ---------------------------------------------------------------------------
// divergence_report.json (consensus.v1) — per-backend accepted solutions
// ---------------------------------------------------------------------------

/// codeprobe's per-backend consensus record, written at `<task>/divergence_report.json`.
#[derive(Deserialize)]
struct DivergenceReport {
    /// `"shipped"` when ≥2 backends agreed above the F1 threshold, else
    /// `"quarantined"`. Only a shipped task has a real consensus answer.
    #[serde(default)]
    decision: String,
    #[serde(default)]
    backend_results: Vec<BackendResult>,
    /// The symbol the question is about (module pair, file-qualified symbol,
    /// or dotted member) — an answer-task oracle-chain reference.
    #[serde(default)]
    symbol: Option<String>,
    /// The file defining the symbol, when the miner recorded one.
    #[serde(default)]
    defining_file: Option<String>,
    /// The agreed consensus file set.
    #[serde(default)]
    consensus_files: Vec<String>,
}

#[derive(Deserialize)]
struct BackendResult {
    #[serde(default)]
    backend: String,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The divergence-report facts the loader consumes: the per-backend accepted
/// solutions plus the answer-task oracle-chain references. Default (empty) for
/// an absent or quarantined report — a task the consensus gate did not ship
/// carries neither native accepted solutions nor a usable oracle chain.
#[derive(Default)]
struct DivergenceFacts {
    accepted_solutions: Vec<AcceptedSolution>,
    consensus_files: BTreeSet<String>,
    defining_file: Option<String>,
    symbol: Option<String>,
}

/// Read `divergence_report.json` into the facts the loader consumes.
///
/// This is codeprobe's authoritative record of independent multi-backend mining
/// (schema `consensus.v1`): each backend's own file-set, plus the `decision`
/// (`shipped` ⇔ ≥2 backends agreed above the F1 threshold). AOA consumes that
/// decision rather than re-deriving agreement: only a `shipped` report yields
/// accepted solutions, and only available, errored-free, named backends
/// contribute. Absent the report (external/probe tasks with no consensus leg),
/// there are simply no native accepted solutions.
///
/// Note: each backend's own `files` are used (the spread edit-locality needs),
/// NOT the report's `consensus_files` (the agreed intersection). `G_t` comes
/// separately from `ground_truth.json`; in a real codeprobe task both derive from
/// one `ConsensusDecision`, so they agree by construction at mine time. The
/// `consensus_files`/`defining_file`/`symbol` fields feed the answer-task
/// oracle chain instead (`OracleChainFacts`).
fn read_divergence(dir: &Path) -> Result<DivergenceFacts, BenchError> {
    let path = dir.join("divergence_report.json");
    if !path.exists() {
        return Ok(DivergenceFacts::default());
    }
    let raw = read_file(&path)?;
    let report: DivergenceReport =
        serde_json::from_str(&raw).map_err(|source| BenchError::Json {
            path: path.clone(),
            source,
        })?;

    if report.decision != "shipped" {
        return Ok(DivergenceFacts::default());
    }

    // Keep every usable backend identity — available, errored-free, named. File
    // emptiness is deliberately NOT filtered here: a shipped report is codeprobe's
    // authoritative "≥2 backends agreed" judgment, so dropping a backend for an
    // empty file-set would let AOA silently demote a shipped consensus to None.
    // Empty solutions are dropped downstream by `accepted_solution_files`, where
    // edit-locality (which needs a real spread) actually cares.
    let accepted_solutions = report
        .backend_results
        .into_iter()
        .filter(|b| b.available && b.error.is_none() && !b.backend.is_empty())
        .map(|b| AcceptedSolution {
            backend: b.backend,
            files: b.files.into_iter().collect(),
        })
        .collect();
    Ok(DivergenceFacts {
        accepted_solutions,
        consensus_files: report.consensus_files.into_iter().collect(),
        defining_file: report.defining_file.filter(|f| !f.trim().is_empty()),
        symbol: report.symbol.filter(|s| !s.trim().is_empty()),
    })
}

/// Keep a `ground_truth_commit` only if it anchors the oracle to a real mined
/// commit.
///
/// A blank value, or the (ASCII) git null object id (all zeros, codeprobe's
/// sanitized / absent-commit placeholder), is dropped: it must NOT grant a task
/// contamination-free `External` provenance it did not earn. The check targets
/// the null sentinel specifically — an otherwise-improbable but well-formed SHA
/// (e.g. all ones) is left alone.
fn meaningful_commit(commit: Option<String>) -> Option<String> {
    // Trim only for the predicate; the original value is stored as-is when kept.
    commit.filter(|c| {
        let t = c.trim();
        !t.is_empty() && !t.chars().all(|ch| ch == '0')
    })
}

/// codeprobe places ground truth under `tests/` in the probe/dual layout and at
/// the task root in the org-scale layout. Prefer whichever exists.
fn ground_truth_dir(dir: &Path) -> PathBuf {
    let tests = dir.join("tests");
    if tests.join("ground_truth.json").exists() {
        tests
    } else {
        dir.to_path_buf()
    }
}

fn parse_ground_truth(path: &Path) -> Result<Option<GroundTruthFile>, BenchError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = read_file(path)?;
    let gt: GroundTruthFile = serde_json::from_str(&raw).map_err(|source| BenchError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Some(gt))
}

/// Largest single codeprobe JSON file read into memory. Task-dir files
/// (metadata.json / task.toml / ground_truth.json / divergence_report.json) and
/// run-dir files (per-trial scoring.json) are all small by nature; this ceiling
/// bounds the bytes held from an attacker-controlled directory without rejecting
/// real input.
pub(crate) const MAX_CODEPROBE_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// Read `path` into a `String`, rejecting anything past [`MAX_CODEPROBE_JSON_BYTES`].
fn read_file(path: &Path) -> Result<String, BenchError> {
    read_capped(path, MAX_CODEPROBE_JSON_BYTES)
}

/// Read `path` into a `String`, rejecting anything past `max` bytes.
///
/// Bounded via [`Read::take`] rather than a pre-read `metadata().len()` check so
/// a file that grows (or a symlink whose target swaps) between stat and read
/// cannot blow past the cap. One byte past the cap is read so an exactly-`max`
/// file is accepted while a larger one is rejected.
pub(crate) fn read_capped(path: &Path, max: u64) -> Result<String, BenchError> {
    let file = fs::File::open(path).map_err(|source| BenchError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut raw = String::new();
    let read = file
        .take(max + 1)
        .read_to_string(&mut raw)
        .map_err(|source| BenchError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if read as u64 > max {
        return Err(BenchError::TooLarge {
            path: path.to_path_buf(),
            max,
        });
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_task_dir_recognizes_both_layouts_and_rejects_a_bare_dir() {
        let dir = std::env::temp_dir().join(format!("aoa-bench-istask-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // A bare directory is not a task dir.
        assert!(!is_task_dir(&dir), "no manifest -> not a task dir");
        // Either manifest makes it a task dir.
        fs::write(dir.join("task.toml"), "").unwrap();
        assert!(is_task_dir(&dir), "task.toml -> task dir");
        fs::remove_file(dir.join("task.toml")).unwrap();
        fs::write(dir.join("metadata.json"), "{}").unwrap();
        assert!(is_task_dir(&dir), "metadata.json -> task dir");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn meaningful_commit_rejects_blank_and_the_git_null_id() {
        // Blank and the all-zeros git null id are dropped...
        assert_eq!(meaningful_commit(None), None);
        assert_eq!(meaningful_commit(Some("   ".into())), None);
        assert_eq!(meaningful_commit(Some("0".repeat(40))), None);
        assert_eq!(meaningful_commit(Some(" 0000000 ".into())), None);
        // ...while a real SHA, or any non-all-zeros placeholder, is kept.
        assert_eq!(
            meaningful_commit(Some("a3c0ffee1234567890abcdef1234567890abcdef".into())).as_deref(),
            Some("a3c0ffee1234567890abcdef1234567890abcdef")
        );
        assert_eq!(
            meaningful_commit(Some("1".repeat(40))).as_deref(),
            Some(&"1".repeat(40)[..])
        );
    }

    #[test]
    fn a_null_commit_does_not_grant_external_provenance() {
        use aoa_domain::HeldOutProvenance;

        let dir = std::env::temp_dir().join(format!("aoa-bench-null-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("metadata.json"),
            r#"{
              "id": "synthetic-000", "repo": "sample/widget",
              "metadata": {"ground_truth_commit": "0000000000000000000000000000000000000000"},
              "verification": {"oracle_type": "file_list", "oracle_answer": ["src/a.py"]}
            }"#,
        )
        .unwrap();
        fs::write(dir.join("instruction.md"), "do the thing").unwrap();

        let task = load_task(&dir).unwrap();
        // The null commit is stripped, so the task is NOT externally composed.
        assert_eq!(task.ground_truth_commit, None);
        assert_ne!(task.held_out_provenance(), HeldOutProvenance::External);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_capped_rejects_over_cap_and_accepts_exactly_cap() {
        let dir = std::env::temp_dir().join(format!("aoa-bench-cap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gt.json");
        fs::write(&path, "0123456789").unwrap(); // 10 bytes

        let err = read_capped(&path, 4).unwrap_err();
        assert!(matches!(err, BenchError::TooLarge { max: 4, .. }));
        assert_eq!(read_capped(&path, 10).unwrap().len(), 10);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shipped_divergence_report_yields_per_backend_accepted_solutions() {
        let dir = std::env::temp_dir().join(format!("aoa-bench-diverge-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("divergence_report.json"),
            r#"{
              "schema_version": "consensus.v1",
              "decision": "shipped",
              "backend_results": [
                {"backend": "ast", "available": true, "files": ["a.py", "b.py"], "error": null},
                {"backend": "treesitter", "available": true, "files": ["a.py", "b.py"], "error": null},
                {"backend": "broken", "available": false, "files": [], "error": "timeout"}
              ]
            }"#,
        )
        .unwrap();

        let facts = read_divergence(&dir).unwrap();
        let backends: Vec<&str> = facts
            .accepted_solutions
            .iter()
            .map(|s| s.backend.as_str())
            .collect();
        // The two agreeing backends are kept (identical files and all); the
        // unavailable/errored one is dropped.
        assert_eq!(backends, vec!["ast", "treesitter"]);

        fs::remove_dir_all(&dir).ok();
    }

    /// Contract test for codeprobe's comprehension consensus artifacts
    /// (R0 provenance track, aoa-s62): a dual comprehension task dir carrying
    /// a two-backend `divergence_report.json` and the mine-time commit in
    /// `tests/ground_truth.json` must classify `NativeComposed`. Fixture
    /// content is copied verbatim (minus whitespace) from a real re-mined
    /// marshmallow task (`comprehension-dependency_analysis-006-20a7339c`).
    #[test]
    fn comprehension_consensus_task_classifies_native_composed() {
        use aoa_domain::HeldOutProvenance;

        let dir = std::env::temp_dir().join(format!("aoa-bench-compr-{}", std::process::id()));
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(
            dir.join("metadata.json"),
            r#"{
              "id": "comprehension-dependency_analysis-006-20a7339c",
              "repo": "marshmallow",
              "metadata": {"ground_truth_commit": "", "enrichment_source": "static_analysis"},
              "verification": {
                "type": "artifact_eval", "verification_mode": "dual",
                "ground_truth_path": "tests/ground_truth.json",
                "answer_schema": "file_list", "scoring_policy": "min",
                "ground_truth_schema_version": "comprehension-v1",
                "oracle_answer": []
              }
            }"#,
        )
        .unwrap();
        fs::write(
            dir.join("instruction.md"),
            "# dependency_analysis: tests.base.validate",
        )
        .unwrap();
        fs::write(
            dir.join("tests").join("ground_truth.json"),
            r#"{
              "answer": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"],
              "answer_type": "file_list",
              "confidence": 0.95,
              "provenance": "deterministic",
              "commit": "cd3cda8b1a9ae740d439538cee7aa8faea58d6b9"
            }"#,
        )
        .unwrap();
        fs::write(
            dir.join("divergence_report.json"),
            r#"{
              "schema_version": "consensus.v1",
              "task_id": "comprehension-dependency_analysis-006-20a7339c",
              "template": "dependency_analysis",
              "symbol": "tests.base.validate",
              "defining_file": "tests/base.py",
              "answer_type": "file_list",
              "threshold": 1.0,
              "mode": "exact_answer",
              "decision": "shipped",
              "backend_results": [
                {"backend": "regex_import_graph", "available": true, "n_files": 3,
                 "files": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"],
                 "error": null,
                 "answer": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"]},
                {"backend": "python_ast_graph", "available": true, "n_files": 3,
                 "files": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"],
                 "error": null,
                 "answer": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"],
                 "detail": null}
              ],
              "pair_metrics": [
                {"backend_a": "regex_import_graph", "backend_b": "python_ast_graph", "exact_match": true}
              ],
              "consensus_files": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"],
              "consensus_answer": ["tests/test_decorators.py", "tests/test_deserialization.py", "tests/test_schema.py"]
            }"#,
        )
        .unwrap();

        let task = load_task(&dir).unwrap();
        // The mine-time commit is read from ground_truth.json (probe-layout
        // field), NOT from the empty executor-pinning metadata field.
        assert_eq!(
            task.ground_truth_commit.as_deref(),
            Some("cd3cda8b1a9ae740d439538cee7aa8faea58d6b9")
        );
        // comprehension-v1 has no expected[] file oracle, so the commit must
        // NOT grant External; the two-backend consensus makes it
        // NativeComposed.
        assert!(task.oracle_files.is_empty());
        let backends: Vec<&str> = task
            .accepted_solutions
            .iter()
            .map(|s| s.backend.as_str())
            .collect();
        assert_eq!(backends, vec!["regex_import_graph", "python_ast_graph"]);
        assert_eq!(
            task.held_out_provenance(),
            HeldOutProvenance::NativeComposed
        );
        // The answer-task oracle-chain references are lifted from the same
        // artifacts: the file_list answer, the consensus files, the defining
        // file, and the symbol.
        assert!(task
            .oracle_chain
            .answer_files
            .contains("tests/test_decorators.py"));
        assert!(task
            .oracle_chain
            .consensus_files
            .contains("tests/test_schema.py"));
        assert_eq!(
            task.oracle_chain.defining_file.as_deref(),
            Some("tests/base.py")
        );
        assert_eq!(
            task.oracle_chain.symbol.as_deref(),
            Some("tests.base.validate")
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quarantined_or_absent_report_yields_no_accepted_solutions() {
        let dir = std::env::temp_dir().join(format!("aoa-bench-quar-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Absent report → empty.
        assert!(read_divergence(&dir).unwrap().accepted_solutions.is_empty());
        // Quarantined report (backends disagreed) → empty facts across the
        // board: no accepted solutions AND no oracle-chain references.
        fs::write(
            dir.join("divergence_report.json"),
            r#"{"decision": "quarantined", "symbol": "pkg.mod", "backend_results": [
                {"backend": "ast", "available": true, "files": ["a.py"], "error": null},
                {"backend": "treesitter", "available": true, "files": ["z.py"], "error": null}
            ]}"#,
        )
        .unwrap();
        let facts = read_divergence(&dir).unwrap();
        assert!(facts.accepted_solutions.is_empty());
        assert_eq!(facts.symbol, None);

        fs::remove_dir_all(&dir).ok();
    }
}
