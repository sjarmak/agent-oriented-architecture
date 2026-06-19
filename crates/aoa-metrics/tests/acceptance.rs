use std::collections::{BTreeMap, BTreeSet};

use aoa_metrics::{
    compute_edit_locality, compute_invariant_discoverability, compute_metrics,
    compute_mutation_surface, compute_retrieval_locality, Confidence, IndexQuality, MetricInput,
    SymbolGraph, TransformMap,
};
use aoa_trace::{Span, SpanSource, SpanType, Trace};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn span(span_type: SpanType, seq: u64, attrs: serde_json::Value) -> Span {
    Span {
        span_type,
        source: SpanSource::Native,
        seq,
        attributes: attrs.as_object().cloned().unwrap_or_default(),
    }
}

fn scip_graph() -> SymbolGraph {
    SymbolGraph {
        nodes: vec!["root".into(), "mid".into(), "leaf".into(), "far".into()],
        edges: vec![
            ("root".into(), "mid".into()),
            ("mid".into(), "leaf".into()),
            ("leaf".into(), "far".into()),
        ],
        writable: set(&["mid", "leaf", "far"]),
        quality: IndexQuality::Scip,
    }
}

fn base_input() -> MetricInput {
    MetricInput {
        trace: Trace { spans: vec![] },
        gold_set: set(&["OrderService"]),
        invariant_set: set(&["invariants::orders"]),
        transform: TransformMap::default(),
        edited_files: set(&["a.rs", "b.rs"]),
        accepted_solutions: vec![set(&["a.rs", "b.rs"]), set(&["a.rs", "c.rs"])],
        graph: scip_graph(),
        k: 2,
        held_out_success: true,
    }
}

// Criterion 1: retrieval-locality emits the three measures and anchors G_t to
// base-repo symbols via transform-map.json, including under a rename.
#[test]
fn retrieval_locality_anchors_gold_through_rename() {
    let trace: Trace = serde_json::from_str(include_str!("fixtures/trace-rename.json")).unwrap();
    let transform: TransformMap =
        serde_json::from_str(include_str!("fixtures/transform-map.json")).unwrap();

    let input = MetricInput {
        trace,
        gold_set: set(&["OrderService"]),
        transform,
        k: 3,
        ..base_input()
    };

    let r = compute_retrieval_locality(&input);

    // Anchored gold is the migrated name, not the raw base name.
    assert!(r.anchored_gold.contains("orders::Service"));
    assert!(!r.anchored_gold.contains("OrderService"));

    // First relevant access is the third access span (search, read, lookup).
    assert_eq!(r.tool_calls_to_first_relevant_artifact, Some(3));
    // "orders::Service" is rank 2 of 3 in the first ranked batch -> MRR = 1/2.
    assert!((r.mrr - 0.5).abs() < 1e-9);
    // One gold hit within top-3 over a gold set of size 1 -> Recall@k = 1.0.
    assert!((r.recall_at_k - 1.0).abs() < 1e-9);
    assert_eq!(r.k, 3);
}

#[test]
fn retrieval_locality_misses_when_only_raw_name_present() {
    // If we do NOT anchor (empty map) and the trace uses the migrated name, the
    // raw base name does not match -> no relevant access.
    let trace: Trace = serde_json::from_str(include_str!("fixtures/trace-rename.json")).unwrap();
    let input = MetricInput {
        trace,
        gold_set: set(&["OrderService"]),
        transform: TransformMap::default(),
        k: 3,
        ..base_input()
    };
    let r = compute_retrieval_locality(&input);
    assert_eq!(r.tool_calls_to_first_relevant_artifact, None);
    assert!((r.mrr - 0.0).abs() < 1e-9);
}

// Criterion 2: edit-locality emits inflation against BOTH an intersection floor
// and a union ceiling of >=2 solutions; floor <= ceiling.
#[test]
fn edit_locality_emits_floor_and_ceiling() {
    let input = MetricInput {
        edited_files: set(&["a.rs", "b.rs", "x.rs"]),
        accepted_solutions: vec![set(&["a.rs", "b.rs"]), set(&["a.rs", "c.rs"])],
        ..base_input()
    };
    let e = compute_edit_locality(&input).unwrap();

    // intersection = {a.rs} size 1; union = {a,b,c} size 3.
    assert_eq!(e.intersection_size, 1);
    assert_eq!(e.union_size, 3);
    assert_eq!(e.f_edit_size, 3);

    // floor against union (3/3 = 1.0), ceiling against intersection (3/1 = 3.0).
    assert!((e.floor_inflation - 1.0).abs() < 1e-9);
    assert!((e.ceiling_inflation - 3.0).abs() < 1e-9);
    assert!(e.floor_inflation <= e.ceiling_inflation);
}

#[test]
fn edit_locality_requires_two_solutions() {
    let input = MetricInput {
        accepted_solutions: vec![set(&["a.rs"])],
        ..base_input()
    };
    assert!(compute_edit_locality(&input).is_err());
}

// Criterion 3: invariant-discoverability is true when an I_t access precedes the
// first write.attempt, false otherwise.
#[test]
fn invariant_discovered_before_write() {
    let input = MetricInput {
        invariant_set: set(&["invariants::orders"]),
        trace: Trace {
            spans: vec![
                span(
                    SpanType::SymbolLookup,
                    1,
                    serde_json::json!({ "symbol": "invariants::orders" }),
                ),
                span(
                    SpanType::WriteAttempt,
                    2,
                    serde_json::json!({ "path": "orders.rs" }),
                ),
            ],
        },
        ..base_input()
    };
    let d = compute_invariant_discoverability(&input);
    assert!(d.accessed_before_first_write);
    assert_eq!(d.first_write_seq, Some(2));
}

#[test]
fn invariant_not_discovered_when_accessed_after_write() {
    let input = MetricInput {
        invariant_set: set(&["invariants::orders"]),
        trace: Trace {
            spans: vec![
                span(
                    SpanType::WriteAttempt,
                    1,
                    serde_json::json!({ "path": "orders.rs" }),
                ),
                span(
                    SpanType::SymbolLookup,
                    2,
                    serde_json::json!({ "symbol": "invariants::orders" }),
                ),
            ],
        },
        ..base_input()
    };
    let d = compute_invariant_discoverability(&input);
    assert!(!d.accessed_before_first_write);
}

// Criterion 4: mutation-surface counts writable files reachable at depth <= k and
// emits integer k and over_approximation: true.
#[test]
fn mutation_surface_counts_reachable_and_emits_k() {
    let input = MetricInput {
        k: 1,
        ..base_input()
    };
    let m = compute_mutation_surface(&input);
    // From roots, depth <= 1 reaches root, mid, leaf, far (each is a root node),
    // writable = {mid, leaf, far} -> 3.
    assert_eq!(m.k, 1);
    assert!(m.over_approximation);
    assert_eq!(m.writable_reachable, 3);
    assert!(m.reachable.contains("mid"));
}

#[test]
fn mutation_surface_respects_depth_bound() {
    // Single root with a chain; bound the search so far nodes are unreachable.
    let graph = SymbolGraph {
        nodes: vec!["root".into()],
        edges: vec![("root".into(), "mid".into()), ("mid".into(), "leaf".into())],
        writable: set(&["mid", "leaf"]),
        quality: IndexQuality::Scip,
    };
    let input = MetricInput {
        graph,
        k: 1,
        ..base_input()
    };
    let m = compute_mutation_surface(&input);
    // depth 1 reaches mid but not leaf.
    assert_eq!(m.writable_reachable, 1);
    assert!(m.reachable.contains("mid"));
    assert!(!m.reachable.contains("leaf"));
}

// Criterion 5: every record carries conditioned_on: held_out, and a
// visible-pass-but-held-out-FAIL task is not counted as success.
#[test]
fn records_conditioned_on_held_out_success() {
    let pass = compute_metrics(&MetricInput {
        held_out_success: true,
        ..base_input()
    })
    .unwrap();
    assert!(pass.counted_as_success);
    // Serialized form carries the literal "held_out".
    let json = serde_json::to_value(&pass).unwrap();
    assert_eq!(json["conditioned_on"], "held_out");
    assert_eq!(json["retrieval_locality"]["conditioned_on"], "held_out");

    let held_out_fail = compute_metrics(&MetricInput {
        held_out_success: false,
        ..base_input()
    })
    .unwrap();
    assert!(!held_out_fail.counted_as_success);
}

// Criterion 6: confidence labeling (R15) and R-silent gating.
#[test]
fn scip_index_is_high_confidence_full_weight_and_eligible() {
    let r = compute_metrics(&base_input()).unwrap();
    assert_eq!(r.confidence, Confidence::High);
    assert!((r.weight - 1.0).abs() < 1e-9);
    assert!(r.repo_eligible_for_r0);
}

#[test]
fn best_effort_index_is_low_confidence_lower_weight() {
    let mut graph = scip_graph();
    graph.quality = IndexQuality::BestEffort;
    let r = compute_metrics(&MetricInput {
        graph,
        ..base_input()
    })
    .unwrap();
    assert_eq!(r.confidence, Confidence::Low);
    assert!(r.weight < 1.0);
    assert!(r.repo_eligible_for_r0);
}

#[test]
fn degraded_index_lowers_weight_disqualifies_r0_and_never_raises_surface() {
    let scip = compute_mutation_surface(&base_input());

    let mut graph = scip_graph();
    graph.quality = IndexQuality::Degraded;
    let degraded_input = MetricInput {
        graph,
        ..base_input()
    };
    let degraded = compute_metrics(&degraded_input).unwrap();

    assert_eq!(degraded.confidence, Confidence::Low);
    assert!((degraded.weight - 0.0).abs() < 1e-9);
    assert!(!degraded.repo_eligible_for_r0);

    // Degraded never improves (raises) the mutation surface.
    assert!(degraded.mutation_surface.writable_reachable <= scip.writable_reachable);
    assert_eq!(degraded.mutation_surface.writable_reachable, 0);
    // It still emits k and over_approximation.
    assert!(degraded.mutation_surface.over_approximation);
}

#[test]
fn transform_map_loads_from_fixture() {
    let transform: TransformMap =
        serde_json::from_str(include_str!("fixtures/transform-map.json")).unwrap();
    let expected: BTreeMap<String, String> = [
        ("OrderService".to_string(), "orders::Service".to_string()),
        (
            "PaymentGateway".to_string(),
            "payments::Gateway".to_string(),
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(transform.base_to_migrated, expected);
}
