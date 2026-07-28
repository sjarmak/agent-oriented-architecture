//! codeprobe adapter for the AOA Toolkit: mined task dirs in, scored run dirs
//! back out.
//!
//! codeprobe mines benchmark tasks from repo history, which makes their held-out
//! leg contamination-free by construction (R0b): the answer was never derived
//! from the visible spec the agent sees. This crate owns both halves of the
//! codeprobe boundary:
//!
//! - **task dirs** ([`load_task`]) — reads a mined task directory into AOA task
//!   inputs (instruction, gold artifact set `G_t`, accepted-solution file-sets,
//!   and a classified held-out provenance) and bridges those into the `aoa-gap`
//!   gap gate and the `aoa-metrics` edit-locality floor/ceiling.
//! - **run dirs** ([`TrialScoring`], [`discover_tasks`], [`aggregate_provenance`])
//!   — the `<run_dir>/<task_id>/scoring.json` wire contract codeprobe writes
//!   after an agent runs, trial discovery over that layout, and the held-out
//!   provenance reduction across a set of trials.
//! - **measurement evidence** ([`MeasurementObservationV1`]) — the canonical
//!   per-arm, per-task, per-seed content-addressed record emitted by
//!   `aoa eval experiment`.
//!
//! Both are external-tool contracts: the field names, the directory layout, and
//! the provenance lattice are load-bearing for whether a result may be
//! certified at all. They live here, at the seam, so that no consumer has to
//! re-derive them and no two consumers can drift.
//!
//! Provenance is surfaced to `aoa-gap` as `External` (a `file_list` oracle
//! anchored to a real ground-truth commit) or `NativeComposed` (two or more
//! independently-mined backends agreed in consensus mining, read from
//! `divergence_report.json`), never `SynthesizedFromVisible`. A task with no
//! independent held-out leg classifies as `None`, which drives `compute_gap` to
//! `Unavailable` (gap:unavailable) rather than fabricating a held-out suite.
//! When fewer than two distinct accepted solutions were mined, edit-locality
//! anchors surface `aoa-metrics`' `InsufficientAcceptedSolutions`, not an invented one.

mod bridge;
mod codeprobe_run;
mod error;
mod loader;
mod measurement_observation;
mod oracle;
mod provenance;
mod task;

pub use bridge::EditLocalityAnchors;
pub use codeprobe_run::{
    aggregate_provenance, discover_tasks, discover_tasks_isolating_names, leg_pass, scoring_path,
    transcript_path, DualLegs, TrialScoring,
};
pub use error::BenchError;
pub use loader::{is_task_dir, load_task};
pub use measurement_observation::{
    AnswerMetricsV1, ArmIdentity, ArtifactDigestSetV1, CalibrationConclusion,
    CalibrationEvidenceV1, CalibrationMethod, EditMetricsV1, ExclusionReasonV1, GitHashAlgorithm,
    GitObjectId, MeasurementMetricsV1, MeasurementObservationV1, MeasurementStateV1, MetricValueV1,
    ObservationError, Sha256Digest, EDIT_LOCALITY_DEFINITION_VERSION,
    INVARIANT_DISCOVERABILITY_DEFINITION_VERSION, MUTATION_SURFACE_DEFINITION_VERSION,
    RETRIEVAL_LOCALITY_DEFINITION_VERSION, TRACE_LOCALITY_DEFINITION_VERSION,
    TRACE_REACH_DEPTH_DEFINITION_VERSION,
};
pub use oracle::OracleChainFacts;
pub use task::{AcceptedSolution, CodeprobeTask};
