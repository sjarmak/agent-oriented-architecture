//! Reward-hacking gap and held-out integrity for the AOA Toolkit.
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
//! All logic here is deterministic mechanism: rates are arithmetic means of
//! per-task booleans and labels are boolean predicates over rate/gap deltas.
//!
//! The vocabulary this gate reasons over — `SubjectKey`, `ExposureStatus`,
//! `HeldOutProvenance`, `RunResult` — belongs to `aoa-domain` and is deliberately
//! *not* re-exported here. Naming a held-out subject must not require depending
//! on the gate that scores it; a convenience re-export would restore exactly the
//! coupling the split removed, and would give one type two import paths.
//!
//! Construct validity (R9c) lives in `aoa-construct`, and the external-outcome
//! corpus that could promote a metric to `gating` lives in `aoa-corpus`. Neither
//! is needed to compute or gate on a gap, so neither is a dependency here.

mod compare;
mod error;
mod gap;

pub use compare::{compare, CompareOutcome, CompareWarning, Label};
pub use error::GapError;
pub use gap::{compute_gap, GapOutcome};
