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

use std::path::{Path, PathBuf};

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
    /// The trial this was read from. Not part of codeprobe's wire shape — it is
    /// the directory name, filled in by [`Self::load`] so that every error this
    /// type raises can name its trial without the caller re-supplying (and
    /// potentially mismatching) the id on each accessor.
    #[serde(skip)]
    task_id: String,
    scorer_family: Option<String>,
    passed_direct: Option<bool>,
    passed_artifact: Option<bool>,
    score_direct: Option<f64>,
    score_artifact: Option<f64>,
    error_direct: Option<String>,
    error_artifact: Option<String>,
}

impl DualScoring {
    /// Read and validate one trial's scoring as a clean dual-verifier result.
    ///
    /// Takes the run dir and trial id rather than a path: the
    /// `<run_dir>/<task_id>/scoring.json` layout is this module's contract, so
    /// callers should not be spelling it out.
    pub fn load(run_dir: &Path, task_id: &str) -> Result<Self, BenchError> {
        let path = scoring_path(run_dir, task_id);
        let raw = read_capped(&path, MAX_CODEPROBE_JSON_BYTES)?;
        let mut scoring: DualScoring =
            serde_json::from_str(&raw).map_err(|source| BenchError::Json { path, source })?;
        scoring.task_id = task_id.to_string();
        scoring.ensure_dual()?;
        Ok(scoring)
    }

    /// Reject anything that is not a clean dual-verifier result: both the
    /// held-out (artifact) and visible (direct) legs must have genuinely run.
    ///
    /// Private — `load` is the only entry point; tests in this module exercise
    /// it directly on hand-built structs.
    fn ensure_dual(&self) -> Result<(), BenchError> {
        if self.scorer_family.as_deref() != Some("dual_composite") {
            return Err(BenchError::NotDualComposite {
                task_id: self.task_id.clone(),
                found: self.scorer_family.clone(),
            });
        }
        for (leg, error) in [
            ("direct (visible)", &self.error_direct),
            ("artifact (held-out)", &self.error_artifact),
        ] {
            if let Some(message) = error {
                return Err(BenchError::ScoringLegErrored {
                    task_id: self.task_id.clone(),
                    leg,
                    message: message.clone(),
                });
            }
        }
        Ok(())
    }

    /// Visible (direct/`test.sh`) outcome — the gameable proxy verifier.
    pub fn visible_success(&self) -> Result<bool, BenchError> {
        self.leg(self.passed_direct, self.score_direct, "direct (visible)")
    }

    /// Held-out (artifact/mined-oracle) outcome — the contamination-free leg.
    pub fn held_out_success(&self) -> Result<bool, BenchError> {
        self.leg(
            self.passed_artifact,
            self.score_artifact,
            "artifact (held-out)",
        )
    }

    fn leg(
        &self,
        passed: Option<bool>,
        score: Option<f64>,
        leg: &'static str,
    ) -> Result<bool, BenchError> {
        leg_pass(passed, score).ok_or_else(|| BenchError::MissingScoringLeg {
            task_id: self.task_id.clone(),
            leg,
        })
    }
}

/// Path to a trial's scoring file. The run-dir layout lives here, not in callers.
///
/// Takes the id as `AsRef<Path>` rather than `&str` so the discovery walk can
/// probe with a raw `OsStr` dirent name. Keeping the walk on this accessor is
/// what stops the layout from being restated at the one call site that cannot
/// hold a `String` yet.
pub fn scoring_path(run_dir: &Path, task_id: impl AsRef<Path>) -> PathBuf {
    run_dir.join(task_id).join("scoring.json")
}

/// Path to a trial's agent transcript. Written only when the agent produced
/// stdout, so callers must tolerate its absence.
pub fn transcript_path(run_dir: &Path, task_id: impl AsRef<Path>) -> PathBuf {
    run_dir.join(task_id).join("agent_output.txt")
}

/// Validate a trial directory name as an addressable task id.
///
/// Split out from the walk so the rule is testable without a filesystem, and on
/// every platform — the only way to *build* a non-UTF-8 dir name in a test is
/// unix-specific, but the rule itself is not.
fn trial_id(run_dir: &Path, name: std::ffi::OsString) -> Result<String, BenchError> {
    name.into_string()
        .map_err(|name| BenchError::TrialNameNotUtf8 {
            run_dir: run_dir.to_path_buf(),
            name,
        })
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
///
/// A trial whose directory name is not valid UTF-8 is an error, and neither a
/// skip nor a lossy id. Skipping shrinks the admitted set with no word said. A
/// lossy id is worse: `to_string_lossy` is not injective, so two distinct
/// dirents can collapse to the same id, and the ids this returns are what
/// `eval falsify build` pairs across arms — two physically different trials
/// matching as an "identical pair" would falsify the very claim that tool
/// makes. Rejecting the name is what makes the returned ids unique by
/// construction, since dirent names are unique within a directory.
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
        // A stat failure is per-entry, so name the entry, not the run dir.
        let file_type = entry.file_type().map_err(|source| BenchError::Io {
            path: entry.path(),
            source,
        })?;
        if !file_type.is_dir() {
            continue;
        }
        // No-follow probes: a symlinked `scoring.json`/`agent_output.txt` must
        // not qualify a dir, or a crafted run dir could point the later capped
        // read at an out-of-tree file. `Path::is_file` follows symlinks; the
        // dir-level guard above does not, and these must match it.
        // Probed with the RAW dirent name, never with a lossy rendering of it: a
        // name that is not valid UTF-8 renders to a path that does not exist, so
        // the trial would drop out of the walk with no error at all.
        let name = entry.file_name();
        if is_regular_file(&scoring_path(run_dir, &name))
            || is_regular_file(&transcript_path(run_dir, &name))
        {
            // The entry IS a trial, so its name has to serve as an addressable
            // id. Validated only after the probe, so a non-UTF-8 name that is
            // not a trial at all stays as ignorable as any other junk dir.
            let id = trial_id(run_dir, name)?;
            // Capped before the push, so the ceiling counts ACCEPTED trials
            // rather than scanned entries.
            if task_ids.len() >= MAX_TRIAL_DIRS {
                return Err(BenchError::TooManyTrialDirs {
                    run_dir: run_dir.to_path_buf(),
                    max: MAX_TRIAL_DIRS,
                });
            }
            task_ids.push(id);
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
            task_id: "t".to_string(),
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
        let err = dual(Some("binary")).ensure_dual().unwrap_err();
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

        let err = errored.ensure_dual().unwrap_err();
        assert!(matches!(err, BenchError::ScoringLegErrored { .. }));
        assert!(err.to_string().contains("artifact (held-out) leg errored"));
    }

    #[test]
    fn leg_falls_back_to_score_threshold() {
        let mut scored = dual(Some("dual_composite"));
        scored.score_direct = Some(1.0);
        scored.score_artifact = Some(0.0);

        assert!(scored.visible_success().unwrap());
        assert!(!scored.held_out_success().unwrap());
    }

    #[test]
    fn a_leg_with_no_signal_at_all_fails_loud() {
        let err = dual(Some("dual_composite")).held_out_success().unwrap_err();
        assert!(matches!(err, BenchError::MissingScoringLeg { .. }));
        assert!(err.to_string().contains("artifact (held-out)"));
    }

    #[test]
    fn leg_pass_distinguishes_no_signal_from_failure() {
        // An explicit `passed_*` wins over the score, and no signal at all is
        // distinct from a failure — the case a `#[serde(default)]` score would
        // silently turn into `false`. (The score-threshold fallback itself is
        // covered through the accessors in `leg_falls_back_to_score_threshold`.)
        assert_eq!(leg_pass(Some(false), Some(1.0)), Some(false));
        assert_eq!(leg_pass(None, None), None);
    }

    #[test]
    fn untrusted_text_cannot_inject_terminal_escapes() {
        // Both the task id (a directory name) and the leg error text come from
        // outside; neither may reach stderr as a live control sequence.
        let mut errored = dual(Some("dual_composite"));
        errored.task_id = "task\u{1b}[2J".to_string();
        errored.error_direct = Some("boom\u{1b}[31mRED".to_string());

        let rendered = errored.ensure_dual().unwrap_err().to_string();
        assert!(
            !rendered.contains('\u{1b}'),
            "raw ESC survived into the error message: {rendered:?}"
        );
        assert!(rendered.contains("\\u{1b}"));

        // The non-dual path renders `scorer_family` from the same untrusted file.
        let rendered = dual(Some("evil\u{1b}[2J"))
            .ensure_dual()
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

        let rendered = DualScoring::load(&base, &ids[0]).unwrap_err().to_string();
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

    #[test]
    fn trial_id_rejects_a_name_that_cannot_round_trip() {
        // The rule itself is platform-independent even though only unix can
        // build such a directory name; the walk that applies it is covered by
        // the `discover_tasks` tests below.
        let run_dir = Path::new("/run");
        assert_eq!(
            trial_id(run_dir, std::ffi::OsString::from("task-a")).unwrap(),
            "task-a"
        );

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;

            let err =
                trial_id(run_dir, std::ffi::OsString::from_vec(b"t\xff".to_vec())).unwrap_err();
            assert!(
                matches!(err, BenchError::TrialNameNotUtf8 { .. }),
                "{err:?}"
            );
            // Two properties, asserted without pinning std's escape spelling
            // (the hex casing of `OsStr`'s `Debug` is not a stable contract, and
            // a test that pinned it would fail as a regression on mere drift).
            // First: nothing renders raw — the message is plain ASCII, so no
            // invalid byte and no terminal escape reaches stderr or the
            // persisted report.
            let rendered = err.to_string();
            assert!(
                rendered.is_ascii() && !rendered.contains('\u{1b}'),
                "raw bytes survived into the message: {rendered:?}"
            );
            // Second: distinct names render distinctly. This is what a lossy
            // rendering could not do, and the reason this is an error at all.
            let other =
                trial_id(run_dir, std::ffi::OsString::from_vec(b"t\xfe".to_vec())).unwrap_err();
            assert_ne!(
                rendered,
                other.to_string(),
                "two distinct invalid names must not render identically"
            );
        }
    }

    /// Build a run dir holding one good trial plus the caller's extra dirents,
    /// each `(name, sentinel)` where a `None` sentinel means "not a trial".
    #[cfg(unix)]
    fn run_dir_with(tag: &str, extra: &[(&[u8], Option<&str>)]) -> PathBuf {
        use std::os::unix::ffi::OsStringExt;

        let base = std::env::temp_dir().join(format!("aoa-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let good = base.join("task-a");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("scoring.json"), "{}").unwrap();

        for (name, sentinel) in extra {
            let dir = base.join(std::ffi::OsString::from_vec(name.to_vec()));
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(file) = sentinel {
                std::fs::write(dir.join(file), "{}").unwrap();
            }
        }
        base
    }

    #[cfg(unix)]
    #[test]
    fn discover_tasks_fails_loud_on_non_utf8_trial_name() {
        // A VALID trial sits alongside the bad one, so what this pins about the
        // pre-fix code is `Ok(["task-a"])` — the silent shrink itself — and not
        // merely a different error variant. A held-out suite that quietly
        // admits fewer trials than the run produced is the one outcome this
        // walk exists to prevent, and the walk is fail-closed: an unusable name
        // aborts rather than returning the partial set.
        //
        // This also covers the lossy-collision hazard the doc comment describes,
        // and there is deliberately no separate test for it: the rejection rule
        // is per-name and unconditional, so two names that both survive it keep
        // their own bytes and cannot collide. A test that distinguished
        // "reject because invalid" from "reject to prevent a collision" is
        // unconstructible once the rule is in place.
        let base = run_dir_with("nonutf8", &[(b"task-\xffbad", Some("scoring.json"))]);

        let err = discover_tasks(&base).unwrap_err();
        assert!(
            matches!(err, BenchError::TrialNameNotUtf8 { .. }),
            "a non-UTF-8 trial name must fail loud, not vanish: {err:?}"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discover_tasks_ignores_a_non_utf8_dir_that_is_not_a_trial() {
        // The probe-then-validate ORDER is load-bearing, and this is what pins
        // it: validating the name before probing the sentinels would turn every
        // stray non-UTF-8 junk dir in an otherwise valid run dir into a hard
        // failure — a behavior change the original never had.
        let base = run_dir_with("nonutf8-junk", &[(b"junk-\xff", None)]);

        assert_eq!(discover_tasks(&base).unwrap(), vec!["task-a".to_string()]);

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
