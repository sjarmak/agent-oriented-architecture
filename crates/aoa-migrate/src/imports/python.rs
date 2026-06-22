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

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use super::{subtract_via_inplace_fix, ImportAdapter, SubtractedFile};
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

/// Removes ruff-certified F401 unused imports via an isolated, config-blind
/// `ruff check --select F401 --fix`.
pub(crate) struct PythonImportAdapter;

impl ImportAdapter for PythonImportAdapter {
    fn id(&self) -> &'static str {
        PYTHON_DEAD_IMPORT_ID
    }

    fn describe(&self) -> &'static str {
        "remove ruff-certified F401 unused imports via an isolated ruff check --fix (strictly subtractive)"
    }

    fn eligibility_note(&self) -> &'static str {
        PYTHON_DEAD_IMPORT_ELIGIBILITY
    }

    fn is_eligible(&self, repo: &Path) -> bool {
        PACKAGING_MARKERS.iter().any(|m| repo.join(m).is_file())
    }

    fn subtract_imports(&self, work: &Path) -> Result<Vec<SubtractedFile>, MigrateError> {
        // 1) Classify the tree against the same single-lint selection. A parse
        // failure here is a LOUD RepoDoesNotCheck before we touch anything.
        classify(work)?;

        // 2) Run ruff's own F401 autofixer in place, then diff the touched files.
        subtract_via_inplace_fix(work, &is_python_file, |w, _files| run_ruff_fix(w))
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

/// Run `ruff check --select F401 --output-format json` (no `--fix`) over the
/// isolated copy and classify the outcome into the shared error taxonomy. Returns
/// `Ok(())` when the tree parses and the only findings (if any) are F401.
fn classify(work: &Path) -> Result<(), MigrateError> {
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
    if diagnostics
        .iter()
        .any(|d| d.get("code").and_then(Value::as_str) == Some(INVALID_SYNTAX))
    {
        return Err(MigrateError::RepoDoesNotCheck {
            stderr: "ruff reported invalid-syntax; the tree does not parse cleanly".to_string(),
        });
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
        assert!(!PythonImportAdapter.is_eligible(dir.path()));
    }

    #[test]
    fn packaging_marker_makes_a_tree_eligible() {
        for marker in PACKAGING_MARKERS {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join(marker), "").unwrap();
            assert!(
                PythonImportAdapter.is_eligible(dir.path()),
                "{marker} should make the tree eligible"
            );
        }
    }
}
