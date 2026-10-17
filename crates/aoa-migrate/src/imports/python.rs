//! Python dead-import adapter: ruff `F401` (unused-import) via an isolated,
//! config-blind `ruff check --select F401 --fix`.
//!
//! `F401` is exactly ruff's unused-import rule (the Pyflakes analogue of rustc's
//! `unused_imports`). The invocation is hardened three ways for construct
//! validity:
//!
//! - `--select F401` restricts BOTH detection and fixing to unused-import only —
//!   unused locals (`F841`) and import order (`I001`) are never touched.
//! - `--isolated` makes ruff ignore any `pyproject.toml` / `ruff.toml` in the
//!   copied tree, so a config that sets `fixable = ["ALL"]` or `extend-select`
//!   cannot widen what gets removed.
//! - `--no-cache` keeps the run a pure function of the source + ruff version.
//!
//! Application uses ruff's own `--fix` (run against the isolated copy) rather than
//! applying ruff's structured edits ourselves: a single import line with two
//! unused names yields two overlapping deletion edits that ruff resolves
//! internally — reimplementing that conflict resolution would be a second, weaker
//! analyzer. The shared engine then diffs the touched files back to the real repo.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::{collect_files, subtract_via_inplace_fix, ImportAdapter, Subtraction};
use crate::error::MigrateError;
use crate::fix::FixProvenance;

const PYTHON_DEAD_IMPORT_ID: &str = "dead-imports-python";

/// ruff's unused-import rule code. The single lint class this adapter acts on.
const F401: &str = "F401";

/// ruff's code for a file it could not parse. Its presence means the tree does
/// not cleanly parse, so subtractivity cannot be certified — a LOUD
/// `RepoDoesNotCheck`, never a silent empty plan.
const INVALID_SYNTAX: &str = "invalid-syntax";

/// Packaging markers that make a tree a Python migration target. A marker is
/// required (mirroring Rust's `Cargo.toml` gate): a bare directory of scripts is
/// out of scope, so we never run ruff on an arbitrary script dump.
const PACKAGING_MARKERS: &[&str] = &["pyproject.toml", "setup.py", "setup.cfg"];

pub(crate) const PYTHON_DEAD_IMPORT_ELIGIBILITY: &str = "ruff F401 (unused-import) removal is a construct-valid, reproducible code-layer treatment only when: \
(1) the unused-import determination is reproducible under the recorded ruff version — `--isolated` ignores any in-repo ruff/pyproject config, so the result depends ONLY on the source and the ruff version captured in provenance (the repo does not, and cannot, pin the analyzer behavior we use); \
(2) imports used only via channels ruff's resolver cannot see are an UNCHECKED exclusion — an `if TYPE_CHECKING:` import referenced solely in a string/forward-ref annotation, or a name bound only through a runtime-conditional `try/except ImportError`, may be reported unused and (wrongly, though still subtractively) removed; ruff preserves the genuinely-referenced cases but cannot see every dynamic one; \
(3) the tree parses cleanly — a file ruff cannot parse is a LOUD RepoDoesNotCheck error, never a silent empty plan. \
Only F401 is selected, so unused locals (F841) and import sorting (I001) are out of scope by construction.";

const PYTHON_DEAD_IMPORT_EXCLUSION_ELIGIBILITY: &str = "ruff F401 (unused-import) removal is a construct-valid, reproducible code-layer treatment only when: \
(1) the unused-import determination is reproducible under the recorded ruff version — `--isolated` ignores any in-repo ruff/pyproject config, so the result depends ONLY on the source and the version captured in provenance; \
(2) imports used only via channels ruff's resolver cannot see remain an UNCHECKED exclusion; \
(3) whole files excluded from both validation and fixing are exactly the files the same isolated baseline invocation reports as invalid-syntax, and their relative paths are recorded; F401 is file-local, so an excluded file cannot affect another file's verdict. \
Only F401 is selected, so unused locals (F841) and import sorting (I001) are out of scope by construction.";

/// Removes ruff-certified F401 unused imports via an isolated, config-blind
/// `ruff check --select F401 --fix`.
pub(crate) struct PythonImportAdapter {
    exclude_baseline_unparseable: bool,
}

impl PythonImportAdapter {
    pub(crate) fn strict() -> Self {
        Self {
            exclude_baseline_unparseable: false,
        }
    }

    pub(crate) fn excluding_baseline_unparseable() -> Self {
        Self {
            exclude_baseline_unparseable: true,
        }
    }
}

impl ImportAdapter for PythonImportAdapter {
    fn id(&self) -> &'static str {
        PYTHON_DEAD_IMPORT_ID
    }

    fn describe(&self) -> &'static str {
        "remove ruff-certified F401 unused imports via an isolated ruff check --fix (strictly subtractive)"
    }

    fn eligibility_note(&self) -> &'static str {
        if self.exclude_baseline_unparseable {
            PYTHON_DEAD_IMPORT_EXCLUSION_ELIGIBILITY
        } else {
            PYTHON_DEAD_IMPORT_ELIGIBILITY
        }
    }

    fn is_eligible(&self, repo: &Path) -> bool {
        PACKAGING_MARKERS.iter().any(|m| repo.join(m).is_file())
    }

    fn ineligible_reason(&self) -> &'static str {
        "no pyproject.toml, setup.py or setup.cfg at the repo root"
    }

    fn examined_unit(&self) -> &'static str {
        "Python source files"
    }

    fn subtract_imports(&self, work: &Path) -> Result<Subtraction, MigrateError> {
        let files = collect_files(work, &is_python_file)?;
        let excluded_paths = classify(work, self.exclude_baseline_unparseable)?;

        let included_files: Vec<PathBuf> = files
            .iter()
            .filter(|path| !excluded_paths.contains(path))
            .cloned()
            .collect();
        let subtracted = if excluded_paths.is_empty() {
            subtract_via_inplace_fix(work, &included_files, || run_ruff_fix(work))?
        } else {
            without_excluded_files(work, &excluded_paths, || {
                classify(work, false)?;
                subtract_via_inplace_fix(work, &included_files, || run_ruff_fix(work))
            })?
        };

        Ok(Subtraction {
            examined: files.len(),
            files: subtracted,
            excluded_paths,
        })
    }

    fn provenance(&self, _repo: &Path) -> Result<Option<FixProvenance>, MigrateError> {
        Ok(Some(FixProvenance {
            fix_id: PYTHON_DEAD_IMPORT_ID.to_string(),
            toolchain: ruff_version()?,
            // `--isolated` means the repo's own config never influences the run;
            // reproducibility is anchored solely by the recorded ruff version, not
            // by anything the repo pins.
            pin_present: false,
        }))
    }
}

fn is_python_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "py")
}

fn classify(work: &Path, exclude_baseline_unparseable: bool) -> Result<Vec<PathBuf>, MigrateError> {
    let output = Command::new("ruff")
        .current_dir(work)
        .args([
            "check",
            "--isolated",
            "--select",
            F401,
            "--no-cache",
            "--output-format",
            "json",
            ".",
        ])
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `ruff`: {source}"),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // ruff exits 0 (no findings) or 1 (findings, incl. F401 or invalid-syntax).
    // A higher exit code is a usage/internal failure: refuse loudly before we try
    // to interpret stdout (which on a usage failure is empty or not our JSON).
    if !matches!(output.status.code(), Some(0) | Some(1)) {
        return Err(MigrateError::BuildFailed { stderr });
    }

    // On a clean run (exit 0) ruff prints `[]`; treat an empty/whitespace stdout as
    // no findings rather than an error.
    let diagnostics: Vec<Value> = match serde_json::from_str(stdout.trim()) {
        Ok(v) => v,
        Err(_) if stdout.trim().is_empty() => Vec::new(),
        Err(e) => {
            return Err(MigrateError::BuildFailed {
                stderr: format!("ruff produced unparseable output ({e}):\n{stderr}"),
            })
        }
    };

    // A file ruff cannot parse cannot be certified subtractive.
    let excluded_paths = invalid_syntax_paths(work, &diagnostics)?;
    if !excluded_paths.is_empty() && !exclude_baseline_unparseable {
        return Err(MigrateError::RepoDoesNotCheck {
            stderr: "ruff reported invalid-syntax; the tree does not parse cleanly".to_string(),
        });
    }

    Ok(excluded_paths)
}

fn invalid_syntax_paths(work: &Path, diagnostics: &[Value]) -> Result<Vec<PathBuf>, MigrateError> {
    let root = std::fs::canonicalize(work).map_err(|source| MigrateError::Io {
        path: work.to_path_buf(),
        source,
    })?;
    let mut paths = BTreeSet::new();
    for diagnostic in diagnostics
        .iter()
        .filter(|value| value.get("code").and_then(Value::as_str) == Some(INVALID_SYNTAX))
    {
        paths.insert(diagnostic_relative_path(work, &root, diagnostic)?);
    }
    Ok(paths.into_iter().collect())
}

fn diagnostic_relative_path(
    work: &Path,
    root: &Path,
    diagnostic: &Value,
) -> Result<PathBuf, MigrateError> {
    let filename = diagnostic
        .get("filename")
        .and_then(Value::as_str)
        .ok_or_else(|| MigrateError::FixCompute {
            message: "ruff invalid-syntax diagnostic did not contain a filename".to_string(),
        })?;
    let reported = Path::new(filename);
    let absolute = if reported.is_absolute() {
        reported.to_path_buf()
    } else {
        work.join(reported)
    };
    let canonical = std::fs::canonicalize(&absolute).map_err(|source| MigrateError::Io {
        path: absolute,
        source,
    })?;
    let relative = canonical
        .strip_prefix(root)
        .map_err(|_| MigrateError::PathOutsideRepo {
            path: canonical.clone(),
            repo: root.to_path_buf(),
        })?;
    if !canonical.is_file() {
        return Err(MigrateError::FixCompute {
            message: format!(
                "ruff invalid-syntax diagnostic named a non-file path: {}",
                relative.display()
            ),
        });
    }
    Ok(relative.to_path_buf())
}

fn without_excluded_files<T>(
    work: &Path,
    excluded_paths: &[PathBuf],
    action: impl FnOnce() -> Result<T, MigrateError>,
) -> Result<T, MigrateError> {
    let storage = tempfile::Builder::new()
        .prefix("aoa-ruff-excluded-")
        .tempdir()
        .map_err(|source| MigrateError::Io {
            path: work.to_path_buf(),
            source,
        })?;
    let moved = stage_excluded_files(work, storage.path(), excluded_paths)?;

    let result = action();
    let restoration = restore_excluded_files(&moved);
    match (result, restoration) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => Err(MigrateError::FixCompute {
            message: format!(
                "ruff exclusion run failed: {error}; restoration also failed: {restore_error}"
            ),
        }),
    }
}

fn stage_excluded_files(
    work: &Path,
    storage: &Path,
    excluded_paths: &[PathBuf],
) -> Result<Vec<(PathBuf, PathBuf)>, MigrateError> {
    let mut moved = Vec::with_capacity(excluded_paths.len());
    for (index, relative) in excluded_paths.iter().enumerate() {
        let source_path = work.join(relative);
        let stored_path = storage.join(index.to_string());
        if let Err(source) = std::fs::rename(&source_path, &stored_path) {
            if let Err(restore_error) = restore_excluded_files(&moved) {
                return Err(MigrateError::FixCompute {
                    message: format!(
                        "failed to exclude {}: {source}; restoration also failed: {restore_error}",
                        relative.display()
                    ),
                });
            }
            return Err(MigrateError::Io {
                path: source_path,
                source,
            });
        }
        moved.push((stored_path, source_path));
    }
    Ok(moved)
}

fn restore_excluded_files(moved: &[(PathBuf, PathBuf)]) -> Result<(), MigrateError> {
    for (stored_path, source_path) in moved.iter().rev() {
        std::fs::rename(stored_path, source_path).map_err(|source| MigrateError::Io {
            path: source_path.clone(),
            source,
        })?;
    }
    Ok(())
}

/// Apply ruff's F401 autofixer in place over the isolated copy. Selection is
/// restricted to F401, so only unused imports are removed; `--isolated` blocks any
/// in-repo config from widening scope.
fn run_ruff_fix(work: &Path) -> Result<(), MigrateError> {
    let output = Command::new("ruff")
        .current_dir(work)
        .args([
            "check",
            "--isolated",
            "--select",
            F401,
            "--fix",
            "--no-cache",
            ".",
        ])
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `ruff`: {source}"),
        })?;

    // After a successful classify, the fix pass should resolve all F401s and exit
    // 0. A non-0/1 exit indicates an unexpected failure mid-fix — surface it.
    if !matches!(output.status.code(), Some(0) | Some(1)) {
        return Err(MigrateError::BuildFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// `ruff --version`, e.g. `ruff 0.15.8` — the analyzer identity recorded as the
/// reproducibility provenance.
fn ruff_version() -> Result<String, MigrateError> {
    let output = Command::new("ruff")
        .arg("--version")
        .output()
        .map_err(|source| MigrateError::ToolchainUnavailable {
            detail: format!("could not run `ruff`: {source}"),
        })?;
    if !output.status.success() {
        return Err(MigrateError::ToolchainUnavailable {
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn non_python_tree_honest_degrades_to_empty() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.py"), "import os\n").unwrap();
        // No pyproject.toml/setup.py/setup.cfg => ineligible.
        assert!(!PythonImportAdapter::strict().is_eligible(dir.path()));
    }

    #[test]
    fn packaging_marker_makes_a_tree_eligible() {
        for marker in PACKAGING_MARKERS {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join(marker), "").unwrap();
            assert!(
                PythonImportAdapter::strict().is_eligible(dir.path()),
                "{marker} should make the tree eligible"
            );
        }
    }
}
