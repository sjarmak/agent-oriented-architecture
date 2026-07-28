//! Zero-write telemetry install and read-only tiered audit for the AOA Toolkit.
//!
//! Two library entrypoints, both safe to run against a working tree:
//!
//! - [`observe`] installs trace logging without touching any tracked file — it
//!   only creates the explicitly-ignored `.aoa/` tree. [`write_trace`] is the
//!   instrumentation path that lands a validated trace under `.aoa/traces/`.
//! - [`audit`] builds a ranked, tiered punch-list grounded in measured numbers
//!   and emits a typed measured/excluded record for every live session.
//!   [`LiveMetricContext`] supplies same-task evidence that ambient traces
//!   cannot infer; without it, sessions remain explicit exclusions and do not
//!   promote behavioral metric modes. The audit writes nothing. [`exit_code`]
//!   maps a report to a process exit code.
//!
//! A later CLI unit drives these entrypoints; this crate builds no binary.

mod audit;
mod error;
mod observe;
mod planes;
mod punch;
mod report;
mod structure;
mod tier;

pub use audit::{
    audit, AuditConfig, LiveExclusionReason, LiveMetricContext, LiveMetricObservation,
    LiveObservationState,
};
pub use error::AuditError;
pub use observe::{
    observe, reject_symlinked_path, reject_symlinked_trace_dir, write_trace, ObserveOutcome,
};
pub use punch::{rank, FindingKind, MeasuredCost, PunchItem};
pub use report::{exit_code, AuditReport};
pub use structure::{
    invariant_sites, navigability_sites, structure_measurements, verification_sites,
    StructureMeasure,
};
pub use tier::{EnforcementPlane, Tier};
