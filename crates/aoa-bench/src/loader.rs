use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::BenchError;
use crate::task::CodeprobeTask;

/// Load a codeprobe task directory into AOA task inputs.
///
/// Supports both observed codeprobe layouts: the rich org-scale form
/// (`metadata.json` carrying the oracle answer and ground-truth commit) and the
/// simple probe form (`task.toml` + `tests/ground_truth.json`). The oracle file
/// set is read from `ground_truth.json` (top-level or under `tests/`), and any
/// `ground_truth_<backend>.json` siblings are read as independently-mined
/// accepted solutions.
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
    let commit = file
        .metadata
        .ground_truth_commit
        .filter(|c| !c.trim().is_empty());
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
    accepted_solutions: Vec<BTreeSet<String>>,
    /// History commit recorded alongside the file-list oracle, if any.
    commit: Option<String>,
}

/// Resolve the oracle file set and the independently-mined accepted solutions.
///
/// The oracle file set comes from the manifest's inline answer when present,
/// otherwise from `ground_truth.json`. Backend-variant ground-truth files
/// (`ground_truth_<backend>.json`) each contribute one accepted-solution set.
/// The canonical oracle set is always included as one accepted solution so a
/// single backend that matches the oracle does not over-count. The canonical
/// file's own `commit`, if any, is returned so a probe-layout task can be
/// recognized as externally composed even without an org-scale metadata block.
fn read_ground_truth(dir: &Path, manifest: &Manifest) -> Result<GroundTruth, BenchError> {
    let gt_dir = ground_truth_dir(dir);

    let canonical = parse_ground_truth(&gt_dir.join("ground_truth.json"))?;
    let oracle_files = match (&manifest.inline_oracle_files, &canonical) {
        (Some(inline), _) => inline.clone(),
        (None, Some(gt)) => gt.expected.iter().cloned().collect(),
        (None, None) => BTreeSet::new(),
    };
    let commit = canonical
        .as_ref()
        .and_then(|gt| gt.commit.clone())
        .filter(|c| !c.trim().is_empty());

    let mut accepted: Vec<BTreeSet<String>> = Vec::new();
    if !oracle_files.is_empty() {
        accepted.push(oracle_files.clone());
    }
    for variant in backend_variant_files(&gt_dir)? {
        if let Some(gt) = parse_ground_truth(&variant)? {
            let set: BTreeSet<String> = gt.expected.into_iter().collect();
            if !set.is_empty() {
                accepted.push(set);
            }
        }
    }

    Ok(GroundTruth {
        oracle_files,
        accepted_solutions: accepted,
        commit,
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

/// List `ground_truth_<backend>.json` siblings in sorted order.
fn backend_variant_files(gt_dir: &Path) -> Result<Vec<PathBuf>, BenchError> {
    let entries = fs::read_dir(gt_dir).map_err(|source| BenchError::Io {
        path: gt_dir.to_path_buf(),
        source,
    })?;
    let mut variants: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| BenchError::Io {
            path: gt_dir.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("ground_truth_") && name.ends_with(".json") {
            variants.push(entry.path());
        }
    }
    variants.sort();
    Ok(variants)
}

fn read_file(path: &Path) -> Result<String, BenchError> {
    fs::read_to_string(path).map_err(|source| BenchError::Io {
        path: path.to_path_buf(),
        source,
    })
}
