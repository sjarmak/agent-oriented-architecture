//! Shared helpers for post-processing a codeprobe run directory.
//!
//! Both `eval run` (per-task process metrics) and `eval r0b` (run-level held-out
//! integrity) walk the same `<run_dir>/<task_id>/{agent_output.txt, scoring.json}`
//! layout codeprobe persists (`core/executor.py::_save_task_artifacts`). The
//! directory-discovery logic lives here so the two commands cannot drift on what
//! counts as a trial.

use std::path::Path;

use anyhow::{bail, Context, Result};

/// List the `<task_id>` subdirectories of the run dir that look like trials.
///
/// A trial dir is identified by EITHER per-trial artifact: codeprobe always
/// writes `scoring.json` but writes `agent_output.txt` only when the agent
/// produced stdout. Keying on either means a trial that is missing its
/// transcript is still discovered — and then fails loud downstream — rather than
/// being silently skipped.
pub(crate) fn discover_tasks(run_dir: &Path) -> Result<Vec<String>> {
    let entries = std::fs::read_dir(run_dir)
        .with_context(|| format!("failed to read codeprobe run dir {}", run_dir.display()))?;

    let mut task_ids: Vec<String> = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", run_dir.display()))?;
        // `DirEntry::file_type` does NOT follow symlinks: a symlinked directory
        // must not pull in per-trial artifacts from outside the run tree.
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat entry in {}", run_dir.display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let dir = entry.path();
        if dir.join("scoring.json").is_file() || dir.join("agent_output.txt").is_file() {
            task_ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    task_ids.sort();

    if task_ids.is_empty() {
        bail!(
            "no task trials found under {}: expected <task_id>/ subdirs with scoring.json \
             or agent_output.txt (point the run dir at a run's config-label directory)",
            run_dir.display()
        );
    }
    Ok(task_ids)
}
