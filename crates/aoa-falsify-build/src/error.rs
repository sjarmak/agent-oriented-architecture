//! The crate's failure vocabulary: one internal enum, one public flattening.
//!
//! [`BuildError`] is what every module inside the crate returns.
//! [`FalsifyBuildError`] is what callers see, and it is a flattened string
//! because the builder persists some nested failures as exclusion reasons — a
//! caller must never have to walk a source chain to recover the useful text.
//!
//! # How the variants were triaged
//!
//! The assembly stages stated 33 string contexts, plus one foreign error that
//! crossed on a bare `?` ([`BuildError::Observation`]). Most are genuinely
//! different failures with different operator remedies and get their own
//! variant. Four groups are the same failure wearing a different noun, and are
//! deliberately merged — recorded here so a later reader can tell a decision
//! from an oversight:
//!
//! - [`BuildError::DuplicateRepoId`] covers both manifest id lists. The two
//!   messages differed only in naming the list, so the list became a field.
//! - [`BuildError::ExposureLedgerUnreadable`] covers both the `symlink_metadata`
//!   and the `read` of the ledger. Their messages were already byte-identical.
//! - [`BuildError::ReusedReplicationInput`] covers a repeated seed and a
//!   repeated run directory. Both mean one draw was read twice, so "stable
//!   across K runs" would be vacuous; the code guarding them treats them as one
//!   reason (aoa-g2g5), and the noun is now a field.
//! - [`BuildError::TaskUnreadable`] covers the three `load_task` call sites.
//!   One failure, three purposes; the purpose is a field.
//!
//! What was NOT merged, and why: the eight remaining exposure-ledger faults
//! send an operator somewhere different each time (fix the path, raise the cap,
//! re-run the scan, rebuild the ledger, re-pin the commit), and the two
//! task-shape/`scip_index` faults are opposite conditions with opposite fixes.
//! Collapsing either group would name the failure without naming the remedy.
//!
//! One message did change, deliberately: [`BuildError::TaskUnreadable`] names
//! the tasks directory on all three call sites. Only the `build` one did
//! before, and a reader of the other two could not tell which tree was
//! searched.
//!
//! # Message shape
//!
//! A variant that wraps a foreign error puts it in `#[source]` and does NOT
//! inline it in its own `Display`. [`BuildError::flattened`] appends the
//! chain, so the rendered message reads `context: cause: cause`. The one
//! exception is [`BuildError::ConventionInputs`], which formats its cause into
//! its own message and exposes no source — preserving what that call site
//! rendered before, where the cause was interpolated rather than chained.

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

use aoa_bench::{BenchError, ObservationError};
use aoa_codeprobe_shim::ShimError;
use aoa_metrics::TraceInputError;
use aoa_scip_graph::ScipGraphError;

use crate::manifest::TaskShape;

/// A self-contained R0 evidence-builder failure.
///
/// The builder records some nested failures as persisted exclusion reasons, so
/// callers must never need to walk a source chain to recover the useful text.
/// The message is therefore flattened once at the public boundary and carries
/// no `#[source]`.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct FalsifyBuildError {
    pub(crate) message: String,
}

impl From<BuildError> for FalsifyBuildError {
    fn from(error: BuildError) -> Self {
        let rendered = error.flattened();
        let mut message = String::with_capacity(rendered.len());
        // The library cannot assume its caller has the CLI's terminal
        // sanitizer. Escape control characters at this boundary while
        // preserving operator-facing punctuation byte-for-byte.
        for character in rendered.chars() {
            if character.is_control() {
                message.extend(character.escape_debug());
            } else {
                message.push(character);
            }
        }
        Self { message }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, FalsifyBuildError>;

/// Every way assembling an R0 falsification input can fail.
#[derive(Debug, Error)]
pub(crate) enum BuildError {
    // ---- manifest declarations (structural; no evidence is read yet) ----
    #[error("manifest {list} contains duplicate repo id: {repo_id}")]
    DuplicateRepoId {
        list: RepoIdList,
        repo_id: DiagnosticRepoId,
    },

    #[error("manifest repo inventory mismatch: missing [{missing}]; unexpected [{unexpected}]")]
    RepoInventoryMismatch {
        missing: DiagnosticRepoIds,
        unexpected: DiagnosticRepoIds,
    },

    #[error(
        "--min-pair-yield is a seed-1 preflight and requires exactly one run per repo with \
         k_runs=1; {detail}. Use the seed-1 manifest from docs/r0_runbook.md Step 3"
    )]
    PairYieldPreflightShape { detail: PairYieldDetail },

    #[error("manifest declares no repos")]
    NoRepos,

    #[error("manifest mixes task shapes ({first:?} and {other:?}); one experiment scores one task shape")]
    MixedTaskShapes { first: TaskShape, other: TaskShape },

    #[error(
        "repo {repo_id}: task_shape \"answer\" requires scip_index (the vendored SCIP JSON \
         index the trace-locality/trace-reach inputs are derived from)"
    )]
    AnswerShapeRequiresIndex { repo_id: String },

    #[error(
        "repo {repo_id}: scip_index is only read for task_shape \"answer\"; declare the shape \
         or drop the index"
    )]
    EditShapeRejectsIndex { repo_id: String },

    // ---- per-repo replication structure ----
    #[error("repo {repo_id}: invalid repo_commit")]
    InvalidRepoCommit {
        repo_id: String,
        #[source]
        source: ObservationError,
    },

    #[error(
        "repo {repo_id}: manifest supplies {supplied} run(s) but k_runs is {k_runs}; each repo \
         needs at least k_runs fixed-seed replications"
    )]
    InsufficientRuns {
        repo_id: String,
        supplied: usize,
        k_runs: u32,
    },

    #[error("repo {repo_id}: {input}")]
    ReusedReplicationInput { repo_id: String, input: ReusedInput },

    #[error(
        "repo {repo_id}: run {run_index} (seed {seed}) admits a different identical-pair set \
         than run 0 (missing {missing:?}, extra {extra:?}); determinism across runs requires \
         identical task identities, not just equal counts"
    )]
    PairSetMismatch {
        repo_id: String,
        run_index: usize,
        seed: u64,
        missing: Vec<String>,
        extra: Vec<String>,
    },

    // ---- reading the mined tasks and the arms' trials ----
    //
    // `BenchError` is boxed in the three variants below. It is 136 bytes on its
    // own, which every `Result` in the crate would otherwise carry on its
    // success path too (`clippy::result_large_err`).
    #[error("failed to discover arm trials in {}", run_dir.display())]
    ArmTrialDiscovery {
        run_dir: PathBuf,
        #[source]
        source: Box<BenchError>,
    },

    #[error("failed to load task {task_id} {purpose} from {}", tasks_dir.display())]
    TaskUnreadable {
        task_id: String,
        purpose: TaskPurpose,
        tasks_dir: PathBuf,
        #[source]
        source: Box<BenchError>,
    },

    #[error("repo {repo_id}: held-out provenance")]
    HeldOutProvenance {
        repo_id: String,
        #[source]
        source: Box<BenchError>,
    },

    #[error(transparent)]
    Observation(#[from] ObservationError),

    // ---- the exposure ledger, which the anti-leakage check rests on ----
    #[error("repo {repo_id}: cannot read exposure ledger {}", path.display())]
    ExposureLedgerUnreadable {
        repo_id: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("repo {repo_id}: exposure ledger {} is not a regular file", path.display())]
    ExposureLedgerNotFile { repo_id: String, path: PathBuf },

    #[error(
        "repo {repo_id}: exposure ledger {} exceeds the {cap} byte evidence cap",
        path.display()
    )]
    ExposureLedgerTooLarge {
        repo_id: String,
        path: PathBuf,
        cap: u64,
    },

    #[error("repo {repo_id}: exposure ledger {} is malformed", path.display())]
    ExposureLedgerMalformed {
        repo_id: String,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "repo {repo_id}: exposure ledger {} has no exposure entry for it; re-run \
         `aoa eval exposure scan --out` against the runs root that holds this repo's trials",
        path.display()
    )]
    ExposureEntryMissing { repo_id: String, path: PathBuf },

    #[error(
        "repo {repo_id}: exposure ledger {} carries more than one entry for it, so no single \
         measured verdict describes this repo",
        path.display()
    )]
    ExposureEntryAmbiguous { repo_id: String, path: PathBuf },

    // Quoted and escaped: a display-hostile value must not reshape the
    // diagnostic it appears in.
    #[error(
        "repo {repo_id}: exposure ledger {} records baseline commit \"{}\", which cannot \
         identify a revision: at least {minimum} hex characters are required",
        path.display(),
        baseline.escape_default()
    )]
    ExposureBaselineUnusable {
        repo_id: String,
        path: PathBuf,
        baseline: String,
        minimum: usize,
    },

    #[error(
        "repo {repo_id}: exposure ledger {} was scanned at baseline commit {baseline} but the \
         manifest declares repo_commit {repo_commit}; the ledger does not describe the \
         revision being measured",
        path.display()
    )]
    ExposureBaselineMismatch {
        repo_id: String,
        path: PathBuf,
        baseline: String,
        repo_commit: String,
    },

    // ---- answer-task convention inputs ----
    #[error("repo {repo_id}: failed to read declared scip_index {}", path.display())]
    ScipIndexUnreadable {
        repo_id: String,
        path: PathBuf,
        #[source]
        source: ScipGraphError,
    },

    #[error(
        "repo {repo_id}: scip_index {} yields no definitions with document paths; answer-task \
         convention inputs cannot be derived from it",
        path.display()
    )]
    ScipIndexEmpty { repo_id: String, path: PathBuf },

    #[error("{arm} arm: cannot read trial transcript {}", path.display())]
    TranscriptUnreadable {
        arm: String,
        path: PathBuf,
        #[source]
        source: ShimError,
    },

    #[error(
        "{arm} arm: trace path(s) [{}] are ambiguous sub-repo-relative suffixes of universe \
         files; the trial's footprint cannot be measured",
        paths.join(", ")
    )]
    AmbiguousTracePaths { arm: String, paths: Vec<String> },

    /// `cause` is deliberately not a `#[source]`: this call site interpolated
    /// the underlying error into its own message rather than chaining it, and
    /// chaining it now would print it twice.
    #[error("{arm} arm: {cause}")]
    ConventionInputs { arm: String, cause: TraceInputError },

    #[error(
        "oracle chain unresolvable: no answer/consensus/defining-file/symbol reference \
         resolves against the scip_index file universe"
    )]
    OracleChainUnresolvable,
}

impl BuildError {
    /// This error and its whole cause chain as one line, `context: cause: cause`.
    ///
    /// Unescaped on purpose. The builder persists some of these as exclusion
    /// reasons, which deliberately keep external text byte-for-byte; escaping
    /// belongs at the public boundary, in the conversion to
    /// [`FalsifyBuildError`].
    pub(crate) fn flattened(&self) -> String {
        let mut rendered = self.to_string();
        let mut cause = std::error::Error::source(self);
        while let Some(source) = cause {
            rendered.push_str(": ");
            rendered.push_str(&source.to_string());
            cause = source.source();
        }
        rendered
    }
}

/// Which manifest list a duplicate repo id was found in.
#[derive(Debug)]
pub(crate) enum RepoIdList {
    Expected,
    Repos,
}

impl fmt::Display for RepoIdList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Expected => "expected_repo_ids",
            Self::Repos => "repos",
        })
    }
}

/// A repo id rendered for a diagnostic: quoted, and escaped so a bidi override
/// or a bracket cannot reshape the message it appears in.
#[derive(Debug)]
pub(crate) struct DiagnosticRepoId(String);

impl DiagnosticRepoId {
    pub(crate) fn new(repo_id: &str) -> Self {
        Self(repo_id.to_string())
    }
}

impl fmt::Display for DiagnosticRepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, r#""{}""#, self.0.escape_default())
    }
}

/// A comma-separated list of [`DiagnosticRepoId`]s.
#[derive(Debug)]
pub(crate) struct DiagnosticRepoIds(Vec<DiagnosticRepoId>);

impl DiagnosticRepoIds {
    pub(crate) fn new<'a>(repo_ids: impl Iterator<Item = &'a str>) -> Self {
        Self(repo_ids.map(DiagnosticRepoId::new).collect())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for DiagnosticRepoIds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, repo_id) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{repo_id}")?;
        }
        Ok(())
    }
}

/// Which side of the seed-1 preflight a manifest failed.
#[derive(Debug)]
pub(crate) enum PairYieldDetail {
    /// The manifest declares more than one run per repo.
    DeclaredRuns { k_runs: u32 },
    /// A repo supplies a number of runs the preflight cannot use.
    SuppliedRuns {
        repo_id: String,
        supplied: usize,
        k_runs: u32,
    },
}

impl fmt::Display for PairYieldDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeclaredRuns { k_runs } => write!(f, "manifest declares k_runs={k_runs}"),
            Self::SuppliedRuns {
                repo_id,
                supplied,
                k_runs,
            } => write!(
                f,
                "repo {repo_id} supplies {supplied} runs and manifest declares k_runs={k_runs}"
            ),
        }
    }
}

/// The replication input a manifest reused across runs.
#[derive(Debug)]
pub(crate) enum ReusedInput {
    Seed(u64),
    RunDirectory(PathBuf),
}

impl fmt::Display for ReusedInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Seed(seed) => write!(
                f,
                "seed {seed} is used by more than one run; each of the k_runs replications must \
                 use a distinct seed"
            ),
            Self::RunDirectory(dir) => write!(
                f,
                "run directory {} is used by more than one run/arm; each replication must read \
                 a distinct run directory",
                dir.display()
            ),
        }
    }
}

/// What a task directory was being loaded for.
#[derive(Debug)]
pub(crate) enum TaskPurpose {
    Oracle,
    Provenance,
}

impl fmt::Display for TaskPurpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Oracle => "oracle",
            Self::Provenance => "provenance",
        })
    }
}
