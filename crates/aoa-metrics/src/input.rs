//! Back-compat re-exports (aoa-00f).
//!
//! `Confidence`, `IndexQuality`, `SymbolGraph`, `TransformMap`, `MetricInput`,
//! and `MetricInputRef` moved down into `aoa-trace` — the substrate layer
//! every crate already depends on — because graph producers (`aoa-scip-graph`)
//! and the falsify gate (`aoa-falsify`) only needed to *name* these types, not
//! link the locality-math extractors that consume them. They are re-exported
//! here unchanged so existing `aoa_metrics::*` paths keep resolving.
pub use aoa_trace::{
    Confidence, IndexQuality, MetricInput, MetricInputRef, SymbolGraph, TransformMap,
};
