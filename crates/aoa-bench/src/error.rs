use std::path::{Path, PathBuf};

use thiserror::Error;

/// Render a path for an error message with control characters escaped.
///
/// Paths reaching these errors are built by joining an attacker-controlled
/// directory name (a codeprobe `<task_id>`) onto a run or task dir, so the
/// rendered path is untrusted text. `Path::display()` does NOT escape control
/// characters — it emits a raw ESC straight to the terminal — and `thiserror`
/// applies `display()` automatically to `Path`/`PathBuf` fields, so every
/// path-carrying variant must route through here instead of interpolating the
/// field directly.
fn escaped_path(path: &Path) -> String {
    path.display().to_string().escape_debug().to_string()
}

/// Errors raised while loading a codeprobe-mined task directory or reading a
/// codeprobe run directory.
#[derive(Debug, Error)]
pub enum BenchError {
    /// The task directory or a required file inside it could not be read.
    #[error("failed to read {}: {source}", escaped_path(.path))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Neither `metadata.json` nor `task.toml` was present, so the directory is
    /// not a recognizable codeprobe task.
    #[error("{} is not a codeprobe task dir: no metadata.json or task.toml", escaped_path(.0))]
    NotATask(PathBuf),

    /// A JSON file in the task dir was malformed.
    #[error("failed to parse {}: {source}", escaped_path(.path))]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The `task.toml` manifest was malformed.
    #[error("failed to parse {}: {source}", escaped_path(.path))]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// A file in the task dir exceeded its byte cap before being read. Guards
    /// against an attacker-controlled task dir feeding an oversized JSON file.
    #[error("{} exceeds {max} byte cap (DoS guard)", escaped_path(.path))]
    TooLarge { path: PathBuf, max: u64 },

    // ---- codeprobe run-dir contract (see `crate::codeprobe_run`) ----
    //
    // The `task_id`, `found` and `message` fields below carry untrusted text
    // (a directory name, values lifted from a `scoring.json`). Never build these
    // variants with unescaped text: put it through `codeprobe_run::escaped`
    // first, for the reason documented there.
    //
    // One exception, by design: `found` is built with `format!("{:?}", ..)`,
    // and `Debug` for `String` already escapes control characters. It is safe
    // as-is and must NOT be double-escaped through `escaped`.
    /// A trial's `scoring.json` was not produced by the dual-verifier scorer, so
    /// it carries no independent held-out leg to gate on.
    #[error(
        "task {task_id}: scoring.json scorer_family is {found}, not \"dual_composite\" — \
         requires a dual-verifier run (held-out artifact leg vs visible direct leg)"
    )]
    NotDualComposite { task_id: String, found: String },

    /// One of the two verifier legs recorded an error, so its outcome is not
    /// trustworthy either way.
    #[error("task {task_id}: {leg} leg errored, cannot trust its outcome: {message}")]
    ScoringLegErrored {
        task_id: String,
        leg: &'static str,
        message: String,
    },

    /// A leg carried neither an explicit `passed_*` nor a `score_*`, which is an
    /// absence of signal rather than a failure.
    #[error(
        "task {task_id}: dual scoring is missing the {leg} leg (no passed_* or score_* field)"
    )]
    MissingScoringLeg { task_id: String, leg: &'static str },

    /// The run directory itself could not be enumerated. Distinct from [`Self::Io`]
    /// so the message still says *what* was being walked: this text reaches the
    /// operator and the persisted `excluded_tasks[].reason` build-report field,
    /// where a bare "failed to read <path>" loses the run-dir context.
    #[error("failed to read codeprobe run dir {}: {source}", escaped_path(.run_dir))]
    RunDirUnreadable {
        run_dir: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The run dir held no recognizable trial subdirectories.
    #[error(
        "no task trials found under {}: expected <task_id>/ subdirs with scoring.json \
         or agent_output.txt (point the run dir at a run's config-label directory)",
        escaped_path(.run_dir)
    )]
    NoTaskTrials { run_dir: PathBuf },

    /// The run dir held more trial subdirectories than the walker accepts.
    #[error(
        "more than {max} task trials under {} (DoS guard): point the run dir at a \
         single run's config-label directory",
        escaped_path(.run_dir)
    )]
    TooManyTrialDirs { run_dir: PathBuf, max: usize },

    /// A task's held-out provenance was synthesized from the visible spec, which
    /// codeprobe never produces — so this is upstream corruption, not an outcome.
    #[error("a task's held-out provenance is synthesized-from-visible — forbidden")]
    SynthesizedProvenance,

    /// Provenance reduction was asked to classify an empty task set, which has no
    /// held-out signal to classify.
    #[error("cannot classify held-out provenance for an empty task set")]
    EmptyProvenanceSet,
}
