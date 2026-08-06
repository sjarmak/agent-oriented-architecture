//! The vocabulary every AOA layer needs to name a held-out subject.
//!
//! Three things live here, and they are here because *every* layer speaks them:
//! held-out subject identity ([`SubjectKey`], [`ExposureStatus`]), where a
//! held-out suite came from ([`HeldOutProvenance`]), and what one evaluation run
//! observed ([`RunResult`], [`TaskOutcome`], [`CanaryItem`]).
//!
//! They were carved out of `aoa-gap`, which had welded them to the gap
//! computation that consumes them. While the two shared a crate, an input-layer
//! crate could not name a subject without depending on a measurement crate —
//! `aoa-bench` did exactly that, and the toolkit's documented
//! inputs → measurement arrow read backwards in the Cargo graph. Splitting them
//! is what lets held-out identity sit behind a stable contract that the gate,
//! the miners, and the reporters can all depend on without depending on each
//! other.
//!
//! This crate is the bottom of the stack: it has no internal dependencies and
//! must never acquire one. It carries wire types and the arithmetic that reads
//! them (pass rates, canary divergence) — no gating, no policy, no judgment.
//! Deciding what a gap *means* is `aoa-gap`'s job.

mod provenance;
mod run;
mod subject;

pub use provenance::HeldOutProvenance;
pub use run::{CanaryItem, RunResult, TaskOutcome};
pub use subject::{ExposureStatus, SubjectKey};
