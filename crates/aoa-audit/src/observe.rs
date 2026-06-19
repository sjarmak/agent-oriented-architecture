use std::path::{Path, PathBuf};

use aoa_trace::{validate_trace, Trace, TraceReport};

use crate::error::AuditError;

/// Where telemetry traces are written, relative to a repo root.
const TRACES_SUBDIR: &str = ".aoa/traces";

/// The result of installing trace telemetry. Tells the caller exactly where
/// traces will be written and where the ignore guard lives.
#[derive(Debug, Clone)]
pub struct ObserveOutcome {
    /// Absolute directory traces are written to (`<repo>/.aoa/traces`).
    pub traces_dir: PathBuf,
    /// The `.gitignore` written under `.aoa/` that ignores everything beneath it.
    pub gitignore: PathBuf,
}

impl ObserveOutcome {
    /// The path a trace named `name` would be written to.
    pub fn trace_path(&self, name: &str) -> PathBuf {
        self.traces_dir.join(name)
    }
}

/// Install trace logging for `repo`. This is a zero-write install with respect
/// to tracked files: it only creates the explicitly-ignored `.aoa/` tree.
///
/// Concretely it creates `<repo>/.aoa/traces/` and writes a `<repo>/.aoa/.gitignore`
/// containing `*`, so every artifact the instrumentation later emits is ignored
/// even in a repo with no top-level ignore for `.aoa/`. No tracked file is
/// touched.
pub fn observe(repo: &Path) -> Result<ObserveOutcome, AuditError> {
    let traces_dir = repo.join(TRACES_SUBDIR);
    std::fs::create_dir_all(&traces_dir).map_err(|source| AuditError::Io {
        path: traces_dir.clone(),
        source,
    })?;

    let gitignore = repo.join(".aoa").join(".gitignore");
    std::fs::write(&gitignore, "*\n").map_err(|source| AuditError::Io {
        path: gitignore.clone(),
        source,
    })?;

    Ok(ObserveOutcome {
        traces_dir,
        gitignore,
    })
}

/// Write a trace through the observe-installed path and validate it in one
/// step. This is the instrumentation entrypoint: the instrumented harness hands
/// a [`Trace`] here and it lands under `.aoa/traces/`, already ordering-checked.
pub fn write_trace(
    outcome: &ObserveOutcome,
    name: &str,
    trace: &Trace,
) -> Result<(PathBuf, TraceReport), AuditError> {
    let path = outcome.trace_path(name);
    let json = serde_json::to_string_pretty(trace).map_err(|source| AuditError::Io {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    std::fs::write(&path, json).map_err(|source| AuditError::Io {
        path: path.clone(),
        source,
    })?;

    let report = validate_trace(&path)?;
    Ok((path, report))
}
