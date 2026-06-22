//! End-to-end tiering tests: a real fixture repo and a vendored SCIP index are
//! indexed into `aoa_metrics::SymbolGraph` artifacts, and each of the three R15
//! quality tiers (Scip / BestEffort / Degraded) is exercised and fed through the
//! `aoa-metrics` extractors. No network service is used — everything reads
//! committed fixtures.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_metrics::{compute_metrics, Confidence, IndexQuality, MetricInput, TransformMap};
use aoa_scip_graph::{
    build_symbol_graph, degraded, index_best_effort, index_with_scip, IndexSource,
};
use aoa_trace::{Span, SpanSource, SpanType, Trace};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn scip_index_path() -> PathBuf {
    fixtures().join("index/index.scip.json")
}

fn repo_dir() -> PathBuf {
    fixtures().join("repo")
}

// --- Tier 1: SCIP-grade index -> high confidence -----------------------------

#[test]
fn scip_index_produces_high_confidence_graph_with_gold_and_invariants() {
    let indexed = index_with_scip(&scip_index_path()).expect("read vendored scip index");

    assert_eq!(indexed.graph.quality, IndexQuality::Scip);
    assert_eq!(indexed.graph.quality.confidence(), Confidence::High);
    assert_eq!(indexed.graph.quality.weight(), 1.0);
    assert!(indexed.graph.quality.eligible_for_r0());

    // Nodes are the four defined symbols; edges follow the reference occurrences.
    let nodes: BTreeSet<&str> = indexed.graph.nodes.iter().map(String::as_str).collect();
    assert_eq!(
        nodes,
        BTreeSet::from([
            "pkg.auth.login",
            "pkg.auth.logout",
            "pkg.tokens.issue_token",
            "pkg.tokens.verify_secret",
        ])
    );
    assert!(indexed
        .graph
        .edges
        .contains(&("pkg.auth.login".into(), "pkg.tokens.issue_token".into())));
    assert!(indexed.graph.edges.contains(&(
        "pkg.tokens.issue_token".into(),
        "pkg.tokens.verify_secret".into()
    )));

    // Writable subset and gold/invariant sets come from the index.
    assert!(indexed.graph.writable.contains("pkg.auth.login"));
    assert!(!indexed.graph.writable.contains("pkg.tokens.issue_token"));
    assert!(indexed.gold_set.contains("pkg.tokens.issue_token"));
    assert!(indexed.invariant_set.contains("pkg.tokens.issue_token"));

    // The writable set is a subset of the nodes.
    for w in &indexed.graph.writable {
        assert!(indexed.graph.nodes.contains(w), "writable {w} not a node");
    }
}

// --- Tier 2: best-effort AST scan -> low confidence --------------------------

#[test]
fn best_effort_scan_produces_low_confidence_graph() {
    let indexed = index_best_effort(&repo_dir()).expect("scan fixture repo");

    assert_eq!(indexed.graph.quality, IndexQuality::BestEffort);
    assert_eq!(indexed.graph.quality.confidence(), Confidence::Low);
    assert_eq!(indexed.graph.quality.weight(), 0.5);
    assert!(indexed.graph.quality.eligible_for_r0());

    let nodes: BTreeSet<&str> = indexed.graph.nodes.iter().map(String::as_str).collect();
    assert_eq!(
        nodes,
        BTreeSet::from([
            "pkg.auth.login",
            "pkg.auth.logout",
            "pkg.tokens.issue_token",
            "pkg.tokens.verify_secret",
        ])
    );

    // The import-and-call edge across modules is recovered.
    assert!(indexed
        .graph
        .edges
        .contains(&("pkg.auth.login".into(), "pkg.tokens.issue_token".into())));
    // The intra-module call edge is recovered.
    assert!(indexed.graph.edges.contains(&(
        "pkg.tokens.issue_token".into(),
        "pkg.tokens.verify_secret".into()
    )));
}

// --- Tier 3: empty / failed -> degraded --------------------------------------

#[test]
fn degraded_sentinel_is_zero_weight_and_r0_ineligible() {
    let indexed = degraded(None);
    assert_eq!(indexed.graph.quality, IndexQuality::Degraded);
    assert_eq!(indexed.graph.quality.weight(), 0.0);
    assert!(!indexed.graph.quality.eligible_for_r0());
    assert!(indexed.graph.nodes.is_empty());
}

#[test]
fn build_symbol_graph_degrades_on_a_missing_index() {
    let indexed = build_symbol_graph(IndexSource::Scip {
        index_path: Path::new("/nonexistent/index.scip.json"),
    });
    assert_eq!(indexed.graph.quality, IndexQuality::Degraded);
    assert!(!indexed.graph.quality.eligible_for_r0());
    // The root cause is preserved rather than silently swallowed: a missing index
    // is distinguishable from a legitimately empty repo.
    let reason = indexed
        .degrade_reason
        .expect("a read failure must record its root cause");
    assert!(
        reason.contains("/nonexistent/index.scip.json"),
        "reason should name the failed path, got: {reason}"
    );
}

#[test]
fn build_symbol_graph_degrades_on_an_empty_repo() {
    let empty = tempdir_with_no_py_files();
    let indexed = build_symbol_graph(IndexSource::BestEffort { repo_dir: &empty });
    assert_eq!(indexed.graph.quality, IndexQuality::Degraded);
    // A clean read that simply found no symbols carries no error reason — the
    // signal that separates "broken index" from "empty repo".
    assert!(indexed.degrade_reason.is_none());
    std::fs::remove_dir_all(&empty).ok();
}

#[test]
fn build_symbol_graph_returns_real_graph_for_a_valid_source() {
    let indexed = build_symbol_graph(IndexSource::Scip {
        index_path: &scip_index_path(),
    });
    assert_eq!(indexed.graph.quality, IndexQuality::Scip);
    assert!(!indexed.graph.nodes.is_empty());
}

// --- Wiring: the produced graph feeds the aoa-metrics extractors -------------

#[test]
fn produced_graph_drives_metric_extractors_across_tiers() {
    let scip = index_with_scip(&scip_index_path()).expect("scip index");

    // A trace that reads the invariant then writes inside the writable surface.
    let trace = Trace {
        spans: vec![
            read_span(1, "pkg.tokens.issue_token"),
            write_span(2, "pkg.auth.login"),
        ],
    };

    let input = MetricInput {
        trace,
        gold_set: scip.gold_set.clone(),
        invariant_set: scip.invariant_set.clone(),
        transform: TransformMap::default(),
        edited_files: BTreeSet::from(["pkg.auth.login".to_string()]),
        accepted_solutions: vec![
            BTreeSet::from(["pkg.auth.login".to_string()]),
            BTreeSet::from(["pkg.auth.login".to_string(), "pkg.auth.logout".to_string()]),
        ],
        graph: scip.graph.clone(),
        k: 2,
        held_out_success: true,
    };

    let record = compute_metrics(input.as_view()).expect("compute metrics on scip graph");
    assert_eq!(record.confidence, Confidence::High);
    assert_eq!(record.weight, 1.0);
    assert!(record.repo_eligible_for_r0);
    assert!(record.invariant_discoverability.accessed_before_first_write);

    // Swapping in a degraded graph zeroes the weight and drops the R0 vote,
    // and never raises the mutation surface.
    let degraded_input = MetricInput {
        graph: degraded(None).graph,
        ..input.clone()
    };
    let degraded_record = compute_metrics(degraded_input.as_view()).expect("compute on degraded");
    assert_eq!(degraded_record.weight, 0.0);
    assert!(!degraded_record.repo_eligible_for_r0);
    assert!(degraded_record.mutation_surface.reachable.is_empty());
}

#[test]
fn transform_map_anchors_gold_set_to_migrated_names() {
    let mut indexed = index_with_scip(&scip_index_path()).expect("scip index");
    // Rename the gold symbol in the migrated repo; the trace uses the new name.
    indexed.gold_set = BTreeSet::from(["pkg.tokens.issue_token".to_string()]);
    let transform = TransformMap {
        base_to_migrated: [(
            "pkg.tokens.issue_token".to_string(),
            "pkg.tokens.mint_token".to_string(),
        )]
        .into_iter()
        .collect(),
    };

    let trace = Trace {
        spans: vec![read_span(1, "pkg.tokens.mint_token")],
    };
    let input = MetricInput {
        trace,
        gold_set: indexed.gold_set,
        invariant_set: BTreeSet::new(),
        transform,
        edited_files: BTreeSet::new(),
        accepted_solutions: vec![BTreeSet::new(), BTreeSet::new()],
        graph: indexed.graph,
        k: 1,
        held_out_success: true,
    };

    let record = compute_metrics(input.as_view()).expect("compute metrics");
    // The renamed gold artifact is matched via the transform map.
    assert_eq!(
        record
            .retrieval_locality
            .tool_calls_to_first_relevant_artifact,
        Some(1)
    );
}

// --- helpers -----------------------------------------------------------------

fn read_span(seq: u64, symbol: &str) -> Span {
    let mut attributes = serde_json::Map::new();
    attributes.insert("symbol".into(), serde_json::json!(symbol));
    Span {
        span_type: SpanType::FileRead,
        source: SpanSource::Native,
        seq,
        attributes,
    }
}

fn write_span(seq: u64, symbol: &str) -> Span {
    let mut attributes = serde_json::Map::new();
    attributes.insert("symbol".into(), serde_json::json!(symbol));
    Span {
        span_type: SpanType::WriteAttempt,
        source: SpanSource::Native,
        seq,
        attributes,
    }
}

fn tempdir_with_no_py_files() -> PathBuf {
    let base = std::env::temp_dir().join(format!("aoa-scip-empty-{}", std::process::id()));
    std::fs::create_dir_all(&base).expect("create empty temp dir");
    base
}
