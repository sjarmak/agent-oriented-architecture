//! Reward-hacking gap, held-out integrity, and construct validity for the AOA Toolkit.
//!
//! This crate is the primary evaluation gate. It computes the visible-vs-held-out
//! success gap (R9) and decides whether an AOA migration is `good`: a migration
//! earns `good` ONLY when its held-out pass rate improves AND the gap holds or
//! reduces — never on a visible-pass plus locality improvement alone.
//!
//! Held-out integrity (R0b) is enforced two ways: a held-out suite synthesized
//! toolkit-side from the visible specs is rejected loudly, and an injected
//! leakage canary fails a comparison when the held-out rate rises without the
//! visible rate moving and a known held-out item flips against its expectation.
//! A benchmark with no native composed held-out suite yields `gap: unavailable`
//! and refuses to label any migration — gating on an absent gap is prohibited.
//!
//! Construct validity (R9c): a metric is `advisory` until a correlation report
//! ties it to at least one external outcome (revert rate, incident count, or
//! review acceptance); only then may it be `gating`.
//!
//! All logic here is deterministic mechanism: rates are arithmetic means of
//! per-task booleans and labels are boolean predicates over rate/gap deltas.

mod compare;
mod construct;
mod error;
mod gap;
mod metric_link;
mod provenance;
mod run;

pub use compare::{compare, CompareOutcome, Label};
pub use construct::{
    classify_metric, CorrelationReport, ExternalOutcome, MetricMode, OutcomeCorrelation,
};
pub use error::GapError;
pub use gap::{compute_gap, GapOutcome};
pub use metric_link::classify_record;
pub use provenance::HeldOutProvenance;
pub use run::{CanaryItem, RunResult, TaskOutcome};
