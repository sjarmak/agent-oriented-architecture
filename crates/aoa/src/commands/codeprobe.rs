//! Shared helpers for post-processing a codeprobe run directory.
//!
//! Both `eval run` (per-task process metrics) and `eval r0b` (run-level held-out
//! integrity) walk the same `<run_dir>/<task_id>/{agent_output.txt, scoring.json}`
//! layout codeprobe persists (`core/executor.py::_save_task_artifacts`). The
//! directory-discovery logic lives here so the two commands cannot drift on what
//! counts as a trial.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use aoa_gap::HeldOutProvenance;

use crate::commands::fsutil::{read_to_string_capped, MAX_TASK_DIRS, MAX_TRIAL_JSON_BYTES};

/// A leg `score_*` at or above this counts as a pass when the explicit
/// `passed_*` boolean is absent (exact-match scorers emit 0.0/1.0).
pub(crate) const SCORE_PASS_THRESHOLD: f64 = 1.0;

/// The subset of codeprobe's flattened `scoring.json` the held-out gates read.
///
/// The dual-verifier scorer merges its `details` onto the top level, so the leg
/// fields sit beside `score`/`passed` (`core/executor.py::_save_task_artifacts`):
/// - **held-out** = the ARTIFACT leg (`passed_artifact`): the agent's `answer.json`
///   vs the mined `ground_truth.json` — the contamination-free oracle.
/// - **visible** = the DIRECT leg (`passed_direct`): `test.sh` run against the
///   agent's diff — the gameable proxy verifier.
///
/// Shared by `eval r0b` (run-level leakage) and `eval experiment` (R0 paired-arm
/// build) so the two cannot drift on what a clean dual result is.
#[derive(Debug, Deserialize)]
pub(crate) struct DualScoring {
    scorer_family: Option<String>,
    passed_direct: Option<bool>,
    passed_artifact: Option<bool>,
    score_direct: Option<f64>,
    score_artifact: Option<f64>,
    error_direct: Option<String>,
    error_artifact: Option<String>,
}

impl DualScoring {
    /// Read and validate a trial's `scoring.json` as a clean dual-verifier result.
    pub(crate) fn load(scoring_path: &Path, task_id: &str) -> Result<Self> {
        let raw = read_to_string_capped(scoring_path, MAX_TRIAL_JSON_BYTES)?;
        let scoring: DualScoring = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", scoring_path.display()))?;
        scoring.ensure_dual(task_id)?;
        Ok(scoring)
    }

    /// Reject anything that is not a clean dual-verifier result: both the
    /// held-out (artifact) and visible (direct) legs must have genuinely run.
    /// Private — `load` is the only entry point; tests in this module exercise it
    /// directly on hand-built structs.
    fn ensure_dual(&self, task_id: &str) -> Result<()> {
        // `task_id` is a directory name and the leg errors come from an untrusted
        // `scoring.json`; escape both so a crafted value cannot inject terminal
        // control sequences when the error surfaces on stderr.
        let task_id = task_id.escape_debug();
        if self.scorer_family.as_deref() != Some("dual_composite") {
            bail!(
                "task {task_id}: scoring.json scorer_family is {:?}, not \"dual_composite\" — \
                 requires a dual-verifier run (held-out artifact leg vs visible direct leg)",
                self.scorer_family
            );
        }
        if let Some(e) = &self.error_direct {
            bail!(
                "task {task_id}: direct (visible) leg errored, cannot trust its outcome: {}",
                e.escape_debug()
            );
        }
        if let Some(e) = &self.error_artifact {
            bail!(
                "task {task_id}: artifact (held-out) leg errored, cannot trust its outcome: {}",
                e.escape_debug()
            );
        }
        Ok(())
    }

    /// Visible (direct/`test.sh`) outcome — the gameable proxy verifier.
    pub(crate) fn visible_success(&self, task_id: &str) -> Result<bool> {
        Self::leg(
            self.passed_direct,
            self.score_direct,
            "direct (visible)",
            task_id,
        )
    }

    /// Held-out (artifact/mined-oracle) outcome — the contamination-free leg.
    pub(crate) fn held_out_success(&self, task_id: &str) -> Result<bool> {
        Self::leg(
            self.passed_artifact,
            self.score_artifact,
            "artifact (held-out)",
            task_id,
        )
    }

    fn leg(passed: Option<bool>, score: Option<f64>, name: &str, task_id: &str) -> Result<bool> {
        match (passed, score) {
            (Some(p), _) => Ok(p),
            (None, Some(s)) => Ok(s >= SCORE_PASS_THRESHOLD),
            (None, None) => bail!(
                "task {}: dual scoring is missing the {name} leg \
                 (no passed_* or score_* field)",
                task_id.escape_debug()
            ),
        }
    }
}

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
        // No-follow probes: a symlinked `scoring.json`/`agent_output.txt` must
        // not qualify a dir, or a crafted run dir could point the later capped
        // read at an out-of-tree file. `Path::is_file` follows symlinks; the
        // dir-level guard above does not, and these must match it.
        if is_regular_file(&dir.join("scoring.json"))
            || is_regular_file(&dir.join("agent_output.txt"))
        {
            if task_ids.len() >= MAX_TASK_DIRS {
                bail!(
                    "more than {} task trials under {} (DoS guard): point the run dir at a \
                     single run's config-label directory",
                    MAX_TASK_DIRS,
                    run_dir.display()
                );
            }
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

/// True only if `path` is a regular file, without following symlinks. A symlink
/// (even one targeting a real file) returns false, so a crafted sentinel cannot
/// pull an out-of-tree path into the trial set.
fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// Reduce per-task held-out provenance into a single provenance for a set of
/// tasks (one run, or one repo's identical-pair set).
///
/// Exhaustive over `HeldOutProvenance` so a forbidden suite can never be
/// laundered into a certifiable one: any `SynthesizedFromVisible` is a hard
/// error, any task with no independent held-out leg (`None`) makes the whole set
/// `gap:unavailable`, an all-`External` set is `External`, and any genuine native
/// agreement is `NativeComposed`. Shared by `eval r0b` (run-level) and
/// `eval experiment` (repo-level eligibility).
pub(crate) fn aggregate_provenance(provenances: &[HeldOutProvenance]) -> Result<HeldOutProvenance> {
    // An empty set has no held-out signal at all; falling through to `External`
    // (the most permissive provenance) would silently certify it, so fail loud.
    if provenances.is_empty() {
        bail!("cannot classify held-out provenance for an empty task set");
    }

    let mut any_synth = false;
    let mut any_none = false;
    let mut any_native = false;
    for p in provenances {
        match p {
            HeldOutProvenance::SynthesizedFromVisible => any_synth = true,
            HeldOutProvenance::None => any_none = true,
            HeldOutProvenance::NativeComposed => any_native = true,
            HeldOutProvenance::External => {}
        }
    }
    if any_synth {
        // A synthesized held-out suite cannot arise from codeprobe data
        // (`aoa_bench::classify_provenance` never emits it), so reaching here
        // means upstream corruption, not a routine outcome. Fail loud.
        bail!("a task's held-out provenance is synthesized-from-visible — forbidden");
    }
    if any_none {
        return Ok(HeldOutProvenance::None);
    }
    if any_native {
        return Ok(HeldOutProvenance::NativeComposed);
    }
    Ok(HeldOutProvenance::External)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_provenance_rejects_synthesized_loud() {
        let err = aggregate_provenance(&[
            HeldOutProvenance::External,
            HeldOutProvenance::SynthesizedFromVisible,
        ])
        .unwrap_err();
        assert!(err.to_string().contains("synthesized-from-visible"));
    }

    #[test]
    fn aggregate_provenance_any_none_makes_set_unavailable() {
        let p =
            aggregate_provenance(&[HeldOutProvenance::External, HeldOutProvenance::None]).unwrap();
        assert_eq!(p, HeldOutProvenance::None);
    }

    #[test]
    fn aggregate_provenance_all_external_is_external() {
        let p = aggregate_provenance(&[HeldOutProvenance::External, HeldOutProvenance::External])
            .unwrap();
        assert_eq!(p, HeldOutProvenance::External);
    }

    #[test]
    fn aggregate_provenance_native_wins_over_external() {
        let p = aggregate_provenance(&[
            HeldOutProvenance::External,
            HeldOutProvenance::NativeComposed,
        ])
        .unwrap();
        assert_eq!(p, HeldOutProvenance::NativeComposed);
    }

    #[test]
    fn aggregate_provenance_empty_fails_loud() {
        assert!(aggregate_provenance(&[]).is_err());
    }

    #[test]
    fn non_dual_scoring_fails_loud() {
        let single = DualScoring {
            scorer_family: Some("binary".to_string()),
            passed_direct: None,
            passed_artifact: None,
            score_direct: None,
            score_artifact: None,
            error_direct: None,
            error_artifact: None,
        };
        let err = single.ensure_dual("t").unwrap_err();
        assert!(err.to_string().contains("dual_composite"));
    }

    #[test]
    fn errored_leg_fails_loud() {
        let errored = DualScoring {
            scorer_family: Some("dual_composite".to_string()),
            passed_direct: Some(true),
            passed_artifact: Some(true),
            score_direct: Some(1.0),
            score_artifact: Some(1.0),
            error_direct: None,
            error_artifact: Some("answer.json missing".to_string()),
        };
        let err = errored.ensure_dual("t").unwrap_err();
        assert!(err.to_string().contains("artifact (held-out) leg errored"));
    }

    #[test]
    fn leg_falls_back_to_score_threshold() {
        let scored = DualScoring {
            scorer_family: Some("dual_composite".to_string()),
            passed_direct: None,
            passed_artifact: None,
            score_direct: Some(1.0),
            score_artifact: Some(0.0),
            error_direct: None,
            error_artifact: None,
        };
        assert!(scored.visible_success("t").unwrap());
        assert!(!scored.held_out_success("t").unwrap());
    }

    #[test]
    fn discover_tasks_finds_real_trial_dirs() {
        let base = std::env::temp_dir().join(format!("aoa-discover-real-{}", std::process::id()));
        let trial = base.join("task-a");
        std::fs::create_dir_all(&trial).unwrap();
        std::fs::write(trial.join("scoring.json"), "{}").unwrap();

        let ids = discover_tasks(&base).unwrap();
        assert_eq!(ids, vec!["task-a".to_string()]);

        std::fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discover_tasks_ignores_symlinked_sentinel() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("aoa-discover-sym-{}", std::process::id()));
        let trial = base.join("task-evil");
        let outside = base.join("outside");
        std::fs::create_dir_all(&trial).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("real_scoring.json");
        std::fs::write(&target, "{}").unwrap();
        // A symlinked scoring.json must NOT qualify the dir as a trial.
        symlink(&target, trial.join("scoring.json")).unwrap();

        let err = discover_tasks(&base).unwrap_err();
        assert!(err.to_string().contains("no task trials found"));

        std::fs::remove_dir_all(&base).ok();
    }
}
