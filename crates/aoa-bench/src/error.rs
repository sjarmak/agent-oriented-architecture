use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Escape control characters out of untrusted text for an error message.
///
/// Every field in this enum that carries text from outside — a codeprobe
/// `<task_id>` (a directory name), values lifted from a `scoring.json`, or a
/// path built by joining one onto a run dir — is rendered through this, at
/// render time, in the `#[error]` attribute. Interpolating such a field
/// directly would emit a raw terminal escape sequence to stderr: `thiserror`
/// renders fields via `Display`, and neither `str` nor `Path::display()`
/// escapes control characters.
///
/// The CLI has its own `output::escape_terminal` for the same hazard on the
/// binary side. These are two render boundaries, not two policies — this one
/// covers what `BenchError` itself prints, and a library crate cannot reach
/// into the binary's.
fn escaped(s: &str) -> String {
    s.escape_debug().to_string()
}

/// [`escaped`] for a path field.
fn escaped_path(path: &Path) -> String {
    escaped(&path.display().to_string())
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
    // `task_id`, `found`, `message` and `name` carry untrusted text (a directory
    // name, values lifted from a `scoring.json`). They are stored RAW and escaped at
    // render time by the `#[error]` attributes below — same rule as the paths
    // above, so the enum has exactly one escaping story and no construct-time
    // invariant for a maintainer to forget on the next field.
    /// A trial's `scoring.json` was not produced by the dual-verifier scorer, so
    /// it carries no independent held-out leg to gate on.
    #[error(
        "task {}: scoring.json scorer_family is {:?}, not \"dual_composite\" — \
         requires a dual-verifier run (held-out artifact leg vs visible direct leg)",
        escaped(.task_id), .found
    )]
    NotDualComposite {
        task_id: String,
        /// Rendered with `{:?}`: `Debug` escapes control characters itself, and
        /// it preserves the `Some("..")`/`None` shape the message reads best with.
        found: Option<String>,
    },

    /// One of the two verifier legs recorded an error, so its outcome is not
    /// trustworthy either way.
    #[error(
        "task {}: {leg} leg errored, cannot trust its outcome: {}",
        escaped(.task_id), escaped(.message)
    )]
    ScoringLegErrored {
        task_id: String,
        leg: &'static str,
        message: String,
    },

    /// A leg carried neither an explicit `passed_*` nor a `score_*`, which is an
    /// absence of signal rather than a failure.
    #[error(
        "task {}: dual scoring is missing the {leg} leg (no passed_* or score_* field)",
        escaped(.task_id)
    )]
    MissingScoringLeg { task_id: String, leg: &'static str },

    /// The run directory itself could not be enumerated. Distinct from [`Self::Io`]
    /// so the message still says *what* was being walked — `eval r0b` surfaces
    /// this straight to the operator with no context of its own, where a bare
    /// "failed to read <path>" would not say it was a codeprobe run dir.
    #[error("failed to read codeprobe run dir {}: {source}", escaped_path(.run_dir))]
    RunDirUnreadable {
        run_dir: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A directory that keys as a trial carries a name that is not valid UTF-8,
    /// so its id cannot round-trip through `String`. Fail loud: skipping it
    /// would shrink the admitted trial set without a word, and admitting it
    /// lossily would let two distinct dirents collapse onto one id.
    #[error(
        "trial dir name {:?} under {} is not valid UTF-8: task ids must round-trip \
         as UTF-8 to address the trial",
        .name, escaped_path(.run_dir)
    )]
    TrialNameNotUtf8 {
        run_dir: PathBuf,
        /// Rendered with `{:?}`, like [`Self::NotDualComposite::found`]: `Debug`
        /// for `OsStr` escapes both control characters and the invalid bytes,
        /// so the operator sees which directory this is. A lossy rendering
        /// would not — that is the whole reason this is an error.
        name: OsString,
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
