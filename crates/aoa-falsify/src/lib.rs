//! Wrong-layer falsification gate (R0) with robust/abstaining hardening (R0')
//! for the AOA Toolkit.
//!
//! Over five or more repos the gate computes, per repo, a `harness-delta` (swap
//! the harness, fix the repo) and a `repo-delta` (apply the AOA migration, fix
//! the harness) on HELD-OUT success over IDENTICAL-PAIR tasks only. The base R0
//! verdict is `proceed` iff repo-delta >= harness-delta on a strict majority of
//! at least five eligible repos; an exact tie defaults to `pivot`.
//!
//! R0' hardens that verdict so it can only be `proceed` when every precondition
//! holds:
//!
//! - DETERMINISM: stable across K >= 3 fixed-seed runs, else `inconclusive`.
//! - CONVENTION-INVARIANCE: invariant across all admissible scoring conventions
//!   for the input's task shape — edit tasks: edit-locality floor/ceiling,
//!   mutation-surface depth-k, alternative metric weights; answer tasks:
//!   trace-locality floor/ceiling, trace-reach depth-k, alternative metric
//!   weights (pre-registered 2026-07-04, bead aoa-dhk.1) — else `inconclusive`.
//!   The conventions are data, not hidden.
//! - ELIGIBILITY: only high-confidence (SCIP-grade) repos with certified
//!   external or natively composed held-out provenance AND calibration vote;
//!   ineligible repos are excluded (R-silent).
//! - POWER: a held-out size and effect-size precondition gates whether a
//!   significant verdict may be returned at all; below threshold `inconclusive`.
//!
//! `inconclusive` is never silently converted to `pivot` — it is preserved
//! verbatim. All logic is deterministic arithmetic and policy enforcement, with
//! the thresholds and conventions emitted as data.

mod convention;
mod delta;
mod eligibility;
mod error;
mod report;
mod types;
mod verdict;

pub use convention::{ConventionFamily, LocalityBound, ScoringConvention};
pub use delta::{repo_deltas, repo_votes_for_proceed, RepoDeltas};
pub use eligibility::is_eligible;
pub use error::FalsifyError;
pub use report::{falsify, FalsifyReport};
pub use types::{
    ConventionInputs, Eligibility, FalsifyConfig, FalsifyInput, PairTask, RepoResult, RepoRun,
    UNREACHABLE_TRACE_REACH_DEPTH,
};
pub use verdict::Verdict;
