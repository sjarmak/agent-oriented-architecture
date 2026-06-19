//! Zero-write telemetry install and read-only tiered audit for the AOA Toolkit.
//!
//! Two library entrypoints, both safe to run against a working tree:
//!
//! - [`observe`] installs trace logging without touching any tracked file — it
//!   only creates the explicitly-ignored `.aoa/` tree. [`write_trace`] is the
//!   instrumentation path that lands a validated trace under `.aoa/traces/`.
//! - [`audit`] builds a ranked, tiered punch-list grounded in measured numbers
//!   (context-file token closure via `aoa-budget`, mutation-surface and
//!   retrieval-locality proxies via `aoa-metrics`, structural enforcement-plane
//!   checks) and renders it both as human text and structured JSON. It writes
//!   nothing. [`exit_code`] maps a report to a process exit code.
//!
//! A later CLI unit drives these entrypoints; this crate builds no binary.

mod audit;
mod error;
mod observe;
mod planes;
mod punch;
mod report;
mod tier;

pub use audit::{audit, AuditConfig};
pub use error::AuditError;
pub use observe::{observe, write_trace, ObserveOutcome};
pub use punch::{rank, MeasuredCost, PunchItem};
pub use report::{exit_code, AuditReport};
pub use tier::{EnforcementPlane, Tier};
