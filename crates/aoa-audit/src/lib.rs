//! Zero-write telemetry install and read-only tiered audit for the AOA Toolkit.
//!
//! Two library entrypoints, both safe to run against a working tree:
//!
//! - [`observe`] installs trace logging without touching any tracked file — it
//!   only creates the explicitly-ignored `.aoa/` tree. [`write_trace`] is the
//!   instrumentation path that lands a validated trace under `.aoa/traces/`.
//! - [`enforcement_liveness`] answers whether the installed runtime enforcement
//!   plane is actually emitting records, in three states rather than two:
//!   enforcing, installed-but-silent, or not installed. [`audit`] carries the
//!   same answer on its report and raises silence as a Tier-1 finding.
//! - [`hook_set_defect`] answers whether an installed plane is the one this
//!   binary writes: behind, ahead, unstamped, or running a wrapper that is gone.
//!   The hook contract it checks against — [`hook_command`] and the constants
//!   beside it — is declared here and composed from by the installer, so the two
//!   sides cannot drift into disagreeing about what an install looks like.
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
mod hook_set;
mod liveness;
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
pub use hook_set::{
    hook_command, hook_set_defect, HookSetDefect, AOA_SETTINGS_KEY, BLOCK_EXIT_CODE,
    ENFORCE_HOOK_SET, ENFORCE_HOOK_SET_VERSION, ENFORCE_WRAPPER_REL, HOOK_VERSION_KEY,
    SETTINGS_REL,
};
pub use liveness::{enforcement_liveness, EnforcementLiveness, Silence};
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
