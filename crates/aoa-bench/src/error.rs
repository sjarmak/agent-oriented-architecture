use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Render external text without terminal-specific transformation.
///
/// Errors also enter persisted JSON evidence, where serde performs the correct
/// wire escaping. Terminal safety belongs to the CLI's final output boundary;
/// escaping here would permanently replace the evidence before it reaches that
/// boundary.
fn raw(s: &str) -> String {
    s.to_string()
}

fn raw_path(path: &Path) -> String {
    path.display().to_string()
}

/// Errors raised while loading a codeprobe-mined task directory or reading a
/// codeprobe run directory.
#[derive(Debug, Error)]
pub enum BenchError {
    /// The task directory or a required file inside it could not be read.
    #[error("failed to read {}: {source}", raw_path(.path))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Neither `metadata.json` nor `task.toml` was present, so the directory is
    /// not a recognizable codeprobe task.
    #[error("{} is not a codeprobe task dir: no metadata.json or task.toml", raw_path(.0))]
    NotATask(PathBuf),

    /// A JSON file in the task dir was malformed.
    #[error("failed to parse {}: {source}", raw_path(.path))]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The `task.toml` manifest was malformed.
    #[error("failed to parse {}: {source}", raw_path(.path))]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// A file in the task dir exceeded its byte cap before being read. Guards
    /// against an attacker-controlled task dir feeding an oversized JSON file.
    #[error("{} exceeds {max} byte cap (DoS guard)", raw_path(.path))]
    TooLarge { path: PathBuf, max: u64 },

    // ---- codeprobe run-dir contract (see `crate::codeprobe_run`) ----
    //
    // `task_id`, `found`, `message` and `name` carry untrusted text (a directory
    // name, values lifted from a `scoring.json`). They remain raw here so JSON
    // evidence is faithful; terminal output escapes the completed message.
    /// A trial's `scoring.json` was not produced by the dual-verifier scorer, so
    /// it carries no independent held-out leg to gate on.
    #[error(
        "task {}: scoring.json scorer_family is {:?}, not \"dual_composite\" — \
         requires a dual-verifier run (held-out artifact leg vs visible direct leg)",
        raw(.task_id), .found
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
        raw(.task_id), raw(.message)
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
        raw(.task_id)
    )]
    MissingScoringLeg { task_id: String, leg: &'static str },

    /// The scorer itself failed before its outcome could be trusted.
    #[error("task {}: scorer errored, cannot trust its outcome: {}", raw(.task_id), raw(.message))]
    ScoringErrored { task_id: String, message: String },

    /// The run directory itself could not be enumerated. Distinct from [`Self::Io`]
    /// so the message still says *what* was being walked — `eval r0b` surfaces
    /// this straight to the operator with no context of its own, where a bare
    /// "failed to read <path>" would not say it was a codeprobe run dir.
    #[error("failed to read codeprobe run dir {}: {source}", raw_path(.run_dir))]
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
        .name, raw_path(.run_dir)
    )]
    TrialNameNotUtf8 {
        run_dir: PathBuf,
        /// `Debug` for `OsStr` escapes the invalid bytes too, not just the
        /// control characters, so the operator sees which directory this is.
        name: OsString,
    },

    /// The run dir held no recognizable trial subdirectories.
    #[error(
        "no task trials found under {}: expected <task_id>/ subdirs with scoring.json \
         or agent_output.txt (point the run dir at a run's config-label directory)",
        raw_path(.run_dir)
    )]
    NoTaskTrials { run_dir: PathBuf },

    /// The run dir held more trial subdirectories than the walker accepts.
    #[error(
        "more than {max} task trials under {} (DoS guard): point the run dir at a \
         single run's config-label directory",
        raw_path(.run_dir)
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

    /// A spent trial could not be mapped to a stable subject. Treating it as
    /// absent would manufacture an all-clear from incomplete evidence.
    #[error("failed to derive exposure subject from {}: {message}", raw_path(.path))]
    ExposureIdentity { path: PathBuf, message: String },

    /// The recursive exposure walk is bounded independently from per-run task
    /// discovery because it includes every arm, seed, and quarantine tree.
    #[error("exposure scan under {} exceeded {max} filesystem entries (DoS guard)", raw_path(.root))]
    TooManyExposureEntries { root: PathBuf, max: usize },

    /// Campaign metadata disagreed on repo identity, so its tasks cannot be
    /// joined to a pinned baseline without guessing.
    #[error("campaign manifests disagree under {}: prep repo {prep_repo:?}, mine repo {mine_repo:?}", raw_path(.dir))]
    ExposureManifestMismatch {
        dir: PathBuf,
        prep_repo: String,
        mine_repo: String,
    },

    /// No pinned admitted corpus was found, so an empty report would not be an
    /// exposure verdict.
    #[error("no campaign prep.json + mine.json pairs found under {}", raw_path(.0))]
    NoExposureCorpora(PathBuf),

    /// Two corpus manifests claimed the same repo. Picking either one would
    /// silently discard a baseline pin and make subject identity ambiguous.
    #[error("more than one campaign corpus claims repository {repo_id:?}")]
    DuplicateExposureRepo { repo_id: String },

    /// Task ids are directory keys, not paths. Reject separators and traversal
    /// before joining an operator-supplied mine manifest to the task root.
    #[error("invalid exposure task id {task_id:?} in {}: expected one path component", raw_path(.mine_path))]
    InvalidExposureTaskId { mine_path: PathBuf, task_id: String },

    /// A prep pin without its admission manifest is incomplete campaign state,
    /// not a corpus that may be omitted from the report.
    #[error("campaign prep manifest {} has no sibling mine.json", raw_path(.0))]
    MissingExposureMine(PathBuf),

    /// An admission manifest without its pinned baseline is equally incomplete.
    #[error("campaign mine manifest {} has no sibling prep.json", raw_path(.0))]
    MissingExposurePrep(PathBuf),

    /// An empty admitted set has no held-out supply to classify.
    #[error("campaign corpus for repository {repo_id:?} has no admitted tasks")]
    EmptyExposureCorpus { repo_id: String },

    /// Filesystem mtimes must serialize as unambiguous Unix timestamps.
    #[error("exposure evidence {} has a modification time before the Unix epoch", raw_path(.0))]
    ExposureMtimeBeforeEpoch(PathBuf),

    /// A platform timestamp exceeded the report wire type.
    #[error("exposure evidence {} has a modification time too large to report", raw_path(.0))]
    ExposureMtimeOverflow(PathBuf),

    /// Internal invariant: a recorded exposure trial always has an evidence mtime.
    #[error("cannot report exposure provenance without evidence-file modification times")]
    EmptyExposureProvenance,
}
