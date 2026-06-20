use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::BenchError;
use crate::task::{AcceptedSolution, CodeprobeTask};

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
    let metadata_path = dir.join("metadata.json");
    let toml_path = dir.join("task.toml");

    let manifest = if metadata_path.exists() {
        read_metadata(&metadata_path)?
    } else if toml_path.exists() {
        read_toml(&toml_path)?
    } else {
        return Err(BenchError::NotATask(dir.to_path_buf()));
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
}

/// The oracle facts resolved from a task's ground-truth files.
struct GroundTruth {
    oracle_files: BTreeSet<String>,
    accepted_solutions: Vec<AcceptedSolution>,
    /// History commit recorded alongside the file-list oracle, if any.
    commit: Option<String>,
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

    let accepted_solutions = read_accepted_backends(dir)?;

    Ok(GroundTruth {
        oracle_files,
        accepted_solutions,
        commit,
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

/// Read the per-backend accepted solutions from `divergence_report.json`.
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
/// one `ConsensusDecision`, so they agree by construction at mine time.
fn read_accepted_backends(dir: &Path) -> Result<Vec<AcceptedSolution>, BenchError> {
    let path = dir.join("divergence_report.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = read_file(&path)?;
    let report: DivergenceReport =
        serde_json::from_str(&raw).map_err(|source| BenchError::Json {
            path: path.clone(),
            source,
        })?;

    if report.decision != "shipped" {
        return Ok(Vec::new());
    }

    // Keep every usable backend identity — available, errored-free, named. File
    // emptiness is deliberately NOT filtered here: a shipped report is codeprobe's
    // authoritative "≥2 backends agreed" judgment, so dropping a backend for an
    // empty file-set would let AOA silently demote a shipped consensus to None.
    // Empty solutions are dropped downstream by `accepted_solution_files`, where
    // edit-locality (which needs a real spread) actually cares.
    let solutions = report
        .backend_results
        .into_iter()
        .filter(|b| b.available && b.error.is_none() && !b.backend.is_empty())
        .map(|b| AcceptedSolution {
            backend: b.backend,
            files: b.files.into_iter().collect(),
        })
        .collect();
    Ok(solutions)
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

/// Largest task-dir file read into memory. metadata.json / task.toml /
/// ground_truth.json / divergence_report.json are small by nature; this ceiling
/// bounds the bytes held from an attacker-controlled task dir without rejecting
/// real input.
const MAX_TASK_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Read `path` into a `String`, rejecting anything past [`MAX_TASK_FILE_BYTES`].
fn read_file(path: &Path) -> Result<String, BenchError> {
    read_capped(path, MAX_TASK_FILE_BYTES)
}

/// Read `path` into a `String`, rejecting anything past `max` bytes.
///
/// Bounded via [`Read::take`] rather than a pre-read `metadata().len()` check so
/// a file that grows (or a symlink whose target swaps) between stat and read
/// cannot blow past the cap. One byte past the cap is read so an exactly-`max`
/// file is accepted while a larger one is rejected.
fn read_capped(path: &Path, max: u64) -> Result<String, BenchError> {
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
        use aoa_gap::HeldOutProvenance;

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

        let solutions = read_accepted_backends(&dir).unwrap();
        let backends: Vec<&str> = solutions.iter().map(|s| s.backend.as_str()).collect();
        // The two agreeing backends are kept (identical files and all); the
        // unavailable/errored one is dropped.
        assert_eq!(backends, vec!["ast", "treesitter"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quarantined_or_absent_report_yields_no_accepted_solutions() {
        let dir = std::env::temp_dir().join(format!("aoa-bench-quar-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Absent report → empty.
        assert!(read_accepted_backends(&dir).unwrap().is_empty());
        // Quarantined report (backends disagreed) → empty, regardless of count.
        fs::write(
            dir.join("divergence_report.json"),
            r#"{"decision": "quarantined", "backend_results": [
                {"backend": "ast", "available": true, "files": ["a.py"], "error": null},
                {"backend": "treesitter", "available": true, "files": ["z.py"], "error": null}
            ]}"#,
        )
        .unwrap();
        assert!(read_accepted_backends(&dir).unwrap().is_empty());

        fs::remove_dir_all(&dir).ok();
    }
}
