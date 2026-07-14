//! Behavior-preserving guarantee for aoa-00f: the substrate types
//! (`Confidence`, `IndexQuality`, `SymbolGraph`, `TransformMap`, `MetricInput`,
//! `MetricInputRef`) moved down into `aoa-trace`, but every existing
//! `aoa_metrics::*` path must still resolve to the *same* type (not a
//! structurally-identical duplicate), so callers who never migrate their
//! imports keep compiling and interoperating with code that imports the
//! `aoa_trace` path directly.

use std::collections::{BTreeMap, BTreeSet};

use aoa_metrics::{
    Confidence, IndexQuality, MetricInput, MetricInputRef, SymbolGraph, TransformMap,
};

#[test]
fn old_paths_name_the_same_types_as_the_new_aoa_trace_home() {
    // If these were merely structurally-identical duplicates rather than a
    // single type re-exported under two paths, these assignments would fail
    // to type-check: a distinct nominal type can't be assigned across a
    // differently-named struct even with identical fields.
    let _: aoa_trace::Confidence = Confidence::High;
    let _: aoa_trace::IndexQuality = IndexQuality::Scip;
    let _: aoa_trace::TransformMap = TransformMap::default();

    let graph: SymbolGraph = SymbolGraph {
        nodes: vec!["a".into()],
        edges: vec![],
        writable: BTreeSet::new(),
        node_paths: BTreeMap::new(),
        quality: IndexQuality::Scip,
    };
    let _: aoa_trace::SymbolGraph = graph;

    let trace = aoa_trace::Trace { spans: vec![] };
    let input: MetricInput = MetricInput {
        trace,
        gold_set: BTreeSet::new(),
        invariant_set: BTreeSet::new(),
        transform: TransformMap::default(),
        edited_files: BTreeSet::new(),
        accepted_solutions: vec![],
        graph: SymbolGraph {
            nodes: vec![],
            edges: vec![],
            writable: BTreeSet::new(),
            node_paths: BTreeMap::new(),
            quality: IndexQuality::Degraded,
        },
        k: 1,
        held_out_success: true,
    };
    let _: aoa_trace::MetricInput = input;

    // MetricInputRef's inherent `as_view` is defined once, in aoa-trace (the
    // orphan rule forbids a second inherent impl elsewhere); calling it via
    // the aoa_metrics-re-exported MetricInput proves the impl travels with
    // the type through the re-export.
    let owned = aoa_trace::MetricInput {
        trace: aoa_trace::Trace { spans: vec![] },
        gold_set: BTreeSet::new(),
        invariant_set: BTreeSet::new(),
        transform: TransformMap::default(),
        edited_files: BTreeSet::new(),
        accepted_solutions: vec![],
        graph: SymbolGraph {
            nodes: vec![],
            edges: vec![],
            writable: BTreeSet::new(),
            node_paths: BTreeMap::new(),
            quality: IndexQuality::BestEffort,
        },
        k: 0,
        held_out_success: false,
    };
    let view: MetricInputRef<'_> = owned.as_view();
    assert_eq!(view.k, 0);
}
