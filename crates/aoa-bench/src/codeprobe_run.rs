//! The codeprobe *run*-directory contract: trial discovery, the `scoring.json`
//! wire shape, and the held-out provenance reduction over a set of trials.
//!
//! Where [`crate::loader`] reads a mined *task* dir (the benchmark input), this
//! module reads what codeprobe writes *after* an agent runs against one:
//! `<run_dir>/<task_id>/{agent_output.txt, scoring.json}`
//! (`core/executor.py::_save_task_artifacts`).
//!
//! `aoa eval r0b`, `aoa eval experiment`, and `aoa eval run` all read this
//! layout; one definition is what stops them drifting on what counts as a trial
//! or as a clean dual result.

use std::path::Path;

use aoa_gap::HeldOutProvenance;
use serde::Deserialize;

use crate::error::BenchError;
use crate::loader::{read_capped, MAX_CODEPROBE_JSON_BYTES};

/// A leg `score_*` at or above this counts as a pass when the explicit
/// `passed_*` boolean is absent (exact-match scorers emit 0.0/1.0).
pub(crate) const SCORE_PASS_THRESHOLD: f64 = 1.0;

/// Largest number of trial subdirectories accepted under one run dir. Bounds the
/// work a crafted run dir of millions of empty subdirs can induce.
///
/// Deliberately distinct from the task-*tree* cap the CLI's corpus walker
/// applies: that one bounds operator-supplied mined tasks, this one bounds
/// trials in an untrusted run dir. Same value today, different threats — they
/// must stay independently tunable.
pub(crate) const MAX_TRIAL_DIRS: usize = 100_000;

/// Decide a single leg's pass/fail from whichever signal `scoring.json` carried.
///
/// `None` means the leg reported *no* signal at all — neither an explicit
/// `passed_*` boolean nor a `score_*` — which is a different thing from a
/// failure and must not be flattened into one. Shared so that every reader of a
/// codeprobe scoring file applies the same rule.
pub fn leg_pass(passed: Option<bool>, score: Option<f64>) -> Option<bool> {
    match (passed, score) {
        (Some(p), _) => Some(p),
        (None, Some(s)) => Some(s >= SCORE_PASS_THRESHOLD),
        (None, None) => None,
    }
}

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
pub struct DualScoring {
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
    pub fn load(scoring_path: &Path, task_id: &str) -> Result<Self, BenchError> {
        let raw = read_capped(scoring_path, MAX_CODEPROBE_JSON_BYTES)?;
        let scoring: DualScoring =
            serde_json::from_str(&raw).map_err(|source| BenchError::Json {
                path: scoring_path.to_path_buf(),
                source,
            })?;
        scoring.ensure_dual(task_id)?;
        Ok(scoring)
    }

    /// Reject anything that is not a clean dual-verifier result: both the
    /// held-out (artifact) and visible (direct) legs must have genuinely run.
    ///
    /// Private — `load` is the only entry point; tests in this module exercise
    /// it directly on hand-built structs.
    fn ensure_dual(&self, task_id: &str) -> Result<(), BenchError> {
        if self.scorer_family.as_deref() != Some("dual_composite") {
            return Err(BenchError::NotDualComposite {
                task_id: escaped(task_id),
                // `{:?}` on the way in: `Debug` for `String` escapes control
                // characters, and this value comes from an untrusted file.
                found: format!("{:?}", self.scorer_family),
            });
        }
        if let Some(e) = &self.error_direct {
            return Err(BenchError::ScoringLegErrored {
                task_id: escaped(task_id),
                leg: "direct (visible)",
                message: escaped(e),
            });
        }
        if let Some(e) = &self.error_artifact {
            return Err(BenchError::ScoringLegErrored {
                task_id: escaped(task_id),
                leg: "artifact (held-out)",
                message: escaped(e),
            });
        }
        Ok(())
    }

    /// Visible (direct/`test.sh`) outcome — the gameable proxy verifier.
    pub fn visible_success(&self, task_id: &str) -> Result<bool, BenchError> {
        Self::leg(
            self.passed_direct,
            self.score_direct,
            "direct (visible)",
            task_id,
        )
    }

    /// Held-out (artifact/mined-oracle) outcome — the contamination-free leg.
    pub fn held_out_success(&self, task_id: &str) -> Result<bool, BenchError> {
        Self::leg(
            self.passed_artifact,
            self.score_artifact,
            "artifact (held-out)",
            task_id,
        )
    }

    fn leg(
        passed: Option<bool>,
        score: Option<f64>,
        name: &'static str,
        task_id: &str,
    ) -> Result<bool, BenchError> {
        leg_pass(passed, score).ok_or_else(|| BenchError::MissingScoringLeg {
            task_id: escaped(task_id),
            leg: name,
        })
    }
}

/// Escape control characters out of untrusted text before it is stored in an
/// error variant.
///
/// The values reaching these errors are a directory name and leg-error strings
/// lifted verbatim from an untrusted `scoring.json`. Escaping happens HERE, at
/// construction, rather than in the `#[error(...)]` format string: `thiserror`
/// renders fields through `Display`, which would emit a raw terminal escape
/// sequence straight to stderr.
fn escaped(s: &str) -> String {
    s.escape_debug().to_string()
}

/// List the `<task_id>` subdirectories of the run dir that look like trials.
///
/// A trial dir is identified by EITHER per-trial artifact: codeprobe always
/// writes `scoring.json` but writes `agent_output.txt` only when the agent
/// produced stdout. Keying on either means a trial that is missing its
/// transcript is still discovered — and then fails loud downstream — rather than
/// being silently skipped.
///
/// The returned ids are sorted; callers (notably the R0 paired-arm build) rely
/// on that ordering being deterministic across arms.
pub fn discover_tasks(run_dir: &Path) -> Result<Vec<String>, BenchError> {
    let entries = std::fs::read_dir(run_dir).map_err(|source| BenchError::RunDirUnreadable {
        run_dir: run_dir.to_path_buf(),
        source,
    })?;

    let mut task_ids: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| BenchError::RunDirUnreadable {
            run_dir: run_dir.to_path_buf(),
            source,
        })?;
        // `DirEntry::file_type` does NOT follow symlinks: a symlinked directory
        // must not pull in per-trial artifacts from outside the run tree.
        // Names the entry, not just the run dir: a stat failure is per-entry,
        // and the original's run-dir-scoped message left the operator with no
        // way to tell which one. Safe to carry the untrusted directory name
        // now that every path-bearing `BenchError` escapes it (`error.rs`).
        let file_type = entry.file_type().map_err(|source| BenchError::Io {
            path: entry.path(),
            source,
        })?;
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
            // Capped before the push, so the ceiling counts ACCEPTED trials
            // rather than scanned entries.
            if task_ids.len() >= MAX_TRIAL_DIRS {
                return Err(BenchError::TooManyTrialDirs {
                    run_dir: run_dir.to_path_buf(),
                    max: MAX_TRIAL_DIRS,
                });
            }
            task_ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    task_ids.sort();

    if task_ids.is_empty() {
        return Err(BenchError::NoTaskTrials {
            run_dir: run_dir.to_path_buf(),
        });
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
pub fn aggregate_provenance(
    provenances: &[HeldOutProvenance],
) -> Result<HeldOutProvenance, BenchError> {
    // An empty set has no held-out signal at all; falling through to `External`
    // (the most permissive provenance) would silently certify it, so fail loud.
    if provenances.is_empty() {
        return Err(BenchError::EmptyProvenanceSet);
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
        // (`classify_provenance` never emits it), so reaching here means
        // upstream corruption, not a routine outcome. Fail loud.
        return Err(BenchError::SynthesizedProvenance);
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
        assert!(matches!(err, BenchError::SynthesizedProvenance));
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
        assert!(matches!(
            aggregate_provenance(&[]).unwrap_err(),
            BenchError::EmptyProvenanceSet
        ));
    }

    fn dual(scorer_family: Option<&str>) -> DualScoring {
        DualScoring {
            scorer_family: scorer_family.map(str::to_string),
            passed_direct: None,
            passed_artifact: None,
            score_direct: None,
            score_artifact: None,
            error_direct: None,
            error_artifact: None,
        }
    }

    #[test]
    fn non_dual_scoring_fails_loud() {
        let err = dual(Some("binary")).ensure_dual("t").unwrap_err();
        assert!(matches!(err, BenchError::NotDualComposite { .. }));
        // `eval r0b` surfaces this string to operators; keep it verbatim.
        assert!(err.to_string().contains("dual_composite"));
    }

    #[test]
    fn errored_leg_fails_loud() {
        let mut errored = dual(Some("dual_composite"));
        errored.passed_direct = Some(true);
        errored.passed_artifact = Some(true);
        errored.error_artifact = Some("answer.json missing".to_string());

        let err = errored.ensure_dual("t").unwrap_err();
        assert!(matches!(err, BenchError::ScoringLegErrored { .. }));
        assert!(err.to_string().contains("artifact (held-out) leg errored"));
    }

    #[test]
    fn leg_falls_back_to_score_threshold() {
        let mut scored = dual(Some("dual_composite"));
        scored.score_direct = Some(1.0);
        scored.score_artifact = Some(0.0);

        assert!(scored.visible_success("t").unwrap());
        assert!(!scored.held_out_success("t").unwrap());
    }

    #[test]
    fn a_leg_with_no_signal_at_all_fails_loud() {
        let err = dual(Some("dual_composite"))
            .held_out_success("t")
            .unwrap_err();
        assert!(matches!(err, BenchError::MissingScoringLeg { .. }));
        assert!(err.to_string().contains("artifact (held-out)"));
    }

    #[test]
    fn leg_pass_distinguishes_no_signal_from_failure() {
        assert_eq!(leg_pass(Some(false), Some(1.0)), Some(false));
        assert_eq!(leg_pass(None, Some(1.0)), Some(true));
        assert_eq!(leg_pass(None, Some(0.0)), Some(false));
        // The case a `#[serde(default)]` score would silently turn into `false`.
        assert_eq!(leg_pass(None, None), None);
    }

    #[test]
    fn untrusted_text_cannot_inject_terminal_escapes() {
        // Both the task id (a directory name) and the leg error text come from
        // outside; neither may reach stderr as a live control sequence.
        let mut errored = dual(Some("dual_composite"));
        errored.error_direct = Some("boom\u{1b}[31mRED".to_string());

        let rendered = errored
            .ensure_dual("task\u{1b}[2J")
            .unwrap_err()
            .to_string();
        assert!(
            !rendered.contains('\u{1b}'),
            "raw ESC survived into the error message: {rendered:?}"
        );
        assert!(rendered.contains("\\u{1b}"));

        // The non-dual path renders `scorer_family` from the same untrusted file.
        let rendered = dual(Some("evil\u{1b}[2J"))
            .ensure_dual("t")
            .unwrap_err()
            .to_string();
        assert!(
            !rendered.contains('\u{1b}'),
            "raw ESC survived via scorer_family: {rendered:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_hostile_trial_dir_name_cannot_inject_escapes_via_the_path() {
        // The escaping above covers free-text fields. A trial DIRECTORY name is
        // also untrusted and reaches errors as part of a joined path — and
        // `Path::display()` does not escape control characters, so the
        // path-carrying variants need their own guard.
        let base = std::env::temp_dir().join(format!("aoa-hostile-name-{}", std::process::id()));
        let trial = base.join("task\u{1b}[2Jevil");
        std::fs::create_dir_all(&trial).unwrap();
        // Keyed as a trial by `agent_output.txt`, with no `scoring.json` — the
        // documented case that sends the caller's read down the Io path.
        std::fs::write(trial.join("agent_output.txt"), "").unwrap();

        let ids = discover_tasks(&base).unwrap();
        assert_eq!(ids.len(), 1);

        let rendered = DualScoring::load(&base.join(&ids[0]).join("scoring.json"), &ids[0])
            .unwrap_err()
            .to_string();
        assert!(
            !rendered.contains('\u{1b}'),
            "raw ESC survived via the path: {rendered:?}"
        );

        std::fs::remove_dir_all(&base).ok();
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
        assert!(matches!(err, BenchError::NoTaskTrials { .. }));

        std::fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discover_tasks_ignores_symlinked_trial_dir() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("aoa-discover-symdir-{}", std::process::id()));
        let outside = base.join("outside").join("real-trial");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("scoring.json"), "{}").unwrap();
        let run_dir = base.join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        // A symlinked trial DIRECTORY must not pull artifacts in from outside
        // the run tree — the `DirEntry::file_type` guard, which the
        // symlinked-file test above does not reach.
        symlink(&outside, run_dir.join("task-evil")).unwrap();

        let err = discover_tasks(&run_dir).unwrap_err();
        assert!(matches!(err, BenchError::NoTaskTrials { .. }));

        std::fs::remove_dir_all(&base).ok();
    }
}
