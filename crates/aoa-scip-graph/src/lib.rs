//! Real symbol-index ingestion for the AOA Toolkit (R15 tiering).
//!
//! Turns a real target repo into an [`aoa_trace::SymbolGraph`] plus the gold
//! set `G_t` and invariant set `I_t` the metric extractors consume. Two index
//! sources are supported, mapped to [`aoa_trace::IndexQuality`]:
//!
//! - A **SCIP-grade** index (precise, tool-emitted) — read from a vendored SCIP
//!   JSON document via [`index_with_scip`] — yields [`IndexQuality::Scip`] and
//!   high-confidence records (R15).
//! - A **best-effort** AST/line scan of the repo source — [`index_best_effort`]
//!   — yields [`IndexQuality::BestEffort`] and low-confidence records.
//!
//! An empty or unusable result is reported as [`IndexQuality::Degraded`]: it
//! lowers the record weight to zero and disqualifies the repo from R0 voting,
//! exactly as `aoa-metrics` already enforces. [`build_symbol_graph`] selects a
//! source and degrades on failure rather than propagating the error into the
//! score.
//!
//! Gold and invariant symbols are emitted as base-repo names; callers anchor
//! them to migrated names through an [`aoa_trace::TransformMap`].

mod best_effort;
mod bounded;
mod error;
mod index;
mod scip;

pub use error::ScipGraphError;
pub use index::{build_symbol_graph, degraded, IndexSource, IndexedRepo};

pub use best_effort::index_best_effort;
pub use scip::index_with_scip;

// Re-export the substrate types this crate produces so callers need one
// import. These now live in aoa-trace (aoa-00f); this crate depends on
// aoa-metrics only for its dev-dependency tests, not for production code.
pub use aoa_trace::{IndexQuality, SymbolGraph, TransformMap};
