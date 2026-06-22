//! codeprobe-mined task loader for the AOA Toolkit.
//!
//! codeprobe mines benchmark tasks from repo history, which makes their held-out
//! leg contamination-free by construction (R0b): the answer was never derived
//! from the visible spec the agent sees. This crate reads a codeprobe task
//! directory into AOA task inputs — instruction, gold artifact set `G_t`,
//! accepted-solution file-sets, and a classified held-out provenance — and
//! bridges those into the `aoa-gap` gap gate and the `aoa-metrics` edit-locality
//! floor/ceiling.
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
mod error;
mod loader;
mod provenance;
mod task;

pub use bridge::EditLocalityAnchors;
pub use error::BenchError;
pub use loader::load_task;
pub use task::{AcceptedSolution, CodeprobeTask};
