//! Trace-derived metric extractors for the AOA Toolkit.
//!
//! Computes the four locality metrics — retrieval-locality, edit-locality,
//! invariant-discoverability, and mutation-surface — from an [`aoa_trace::Trace`]
//! plus a transform map and a SCIP-style symbol graph. Every record is
//! conditioned on held-out success, labeled `high-confidence` for a SCIP-grade
//! index versus `low-confidence` for a best-effort one (R15), and a degraded or
//! empty index lowers the record's weight, never raises the mutation surface,
//! and disqualifies the repo from R0 voting (R-silent).
//!
//! All thresholds — the Recall@k `k`, the mutation-surface depth `k` — are
//! emitted as data, not hidden inside the computation.

mod common;
mod edit;
mod error;
mod input;
mod invariant;
mod mutation;
mod record;
mod retrieval;

pub use common::ConditionedOn;
pub use edit::{compute_edit_locality, EditLocality};
pub use error::MetricError;
pub use input::{Confidence, IndexQuality, MetricInput, MetricInputRef, SymbolGraph, TransformMap};
pub use invariant::{compute_invariant_discoverability, InvariantDiscoverability};
pub use mutation::{compute_mutation_surface, MutationSurface};
pub use record::{compute_metrics, MetricRecord};
pub use retrieval::{compute_retrieval_locality, RetrievalLocality};
