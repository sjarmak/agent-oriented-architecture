//! Integration tests: parse committed sanitized stream-json fixtures and assert
//! the emitted trace is valid, ordered, targeted, and correctly mapped.
//!
//! These run WITHOUT codeprobe present — the fixtures are checked in.

use std::path::Path;

use aoa_codeprobe_shim::parse_transcript_file;
use aoa_trace::{validate_trace_value, SpanSource, SpanType};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn parses_fixture_into_valid_native_trace() {
    let result = parse_transcript_file(&fixture("agent_output.txt")).expect("read fixture");

    // AC1: emitted trace passes validate_trace.
    validate_trace_value(&result.trace).expect("trace validates");

    // AC4: every span is source = native.
    for span in &result.trace.spans {
        assert_eq!(
            span.source,
            SpanSource::Native,
            "span {span:?} must be native"
        );
    }
}

#[test]
fn spans_preserve_order_and_monotonic_seq() {
    // AC2 (order): seq is strictly increasing in emission order.
    let result = parse_transcript_file(&fixture("agent_output.txt")).expect("read fixture");
    let seqs: Vec<u64> = result.trace.spans.iter().map(|s| s.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(seqs, sorted, "seq must be non-decreasing");
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "seq must be strictly increasing"
    );
}

#[test]
fn fixture_covers_each_mapping_case_in_order() {
    // AC2 (targets) + AC3 (mapping coverage) + write.blocked.
    let result = parse_transcript_file(&fixture("agent_output.txt")).expect("read fixture");
    let spans = &result.trace.spans;

    // Expected span types in transcript order. The unknown tool + non-test Bash
    // produce no span; the denied Write becomes write.blocked.
    let types: Vec<SpanType> = spans.iter().map(|s| s.span_type).collect();
    assert_eq!(
        types,
        vec![
            SpanType::RetrievalSearch, // Grep
            SpanType::FileRead,        // Read
            SpanType::GatewayInvoke,   // mcp__codegraph__codegraph_search
            SpanType::WriteBlocked,    // Write -> denied
            SpanType::WriteAttempt,    // Edit
            SpanType::TestRun,         // Bash pytest
        ],
        "mapping/order mismatch: {types:?}"
    );

    // Targets carried in attributes.
    assert_eq!(
        spans[0].attributes.get("query").and_then(|v| v.as_str()),
        Some("fn parse_config")
    );
    assert_eq!(
        spans[1].attributes.get("path").and_then(|v| v.as_str()),
        Some("src/config.rs")
    );
    assert_eq!(
        spans[2].attributes.get("tool").and_then(|v| v.as_str()),
        Some("mcp__codegraph__codegraph_search")
    );
    assert_eq!(
        spans[3].attributes.get("path").and_then(|v| v.as_str()),
        Some("/etc/forbidden.conf")
    );
    assert_eq!(
        spans[4].attributes.get("path").and_then(|v| v.as_str()),
        Some("src/config.rs")
    );
    assert_eq!(
        spans[5].attributes.get("command").and_then(|v| v.as_str()),
        Some("pytest tests/test_config.py -q")
    );
}

#[test]
fn unknown_tools_recorded_as_warnings_not_swallowed() {
    // AC5: unmapped tool names are surfaced, never silently dropped.
    let result = parse_transcript_file(&fixture("agent_output.txt")).expect("read fixture");
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("WeirdCustomTool")),
        "expected a warning for the unknown tool, got {:?}",
        result.warnings
    );
}

#[test]
fn no_symbol_lookup_emitted_pre_graph() {
    // symbol.lookup is documented-absent until the SCIP graph join lands.
    let result = parse_transcript_file(&fixture("agent_output.txt")).expect("read fixture");
    assert!(
        result
            .trace
            .spans
            .iter()
            .all(|s| s.span_type != SpanType::SymbolLookup),
        "symbol.lookup should not be emitted yet"
    );
}

#[test]
fn no_write_trial_ends_with_abstain() {
    // A transcript with no Edit/Write ends with an abstain span.
    let result = parse_transcript_file(&fixture("agent_output_abstain.txt")).expect("read fixture");
    validate_trace_value(&result.trace).expect("trace validates");

    let last = result.trace.spans.last().expect("at least one span");
    assert_eq!(last.span_type, SpanType::Abstain);
    assert!(
        result.trace.spans.iter().all(|s| s.span_type != SpanType::WriteAttempt
            && s.span_type != SpanType::WriteBlocked),
        "abstain trial must not contain write spans"
    );
}

#[test]
fn blocked_only_trial_has_no_abstain_and_only_write_blocked() {
    // A transcript whose ONLY write is denied: the write.attempt sets `saw_write`
    // before the errored tool_result reclassifies it to write.blocked, so NO
    // trailing abstain is appended. This locks the "denied write is still a write"
    // edge — a blocked attempt is distinct from an abstain, never collapsed into one.
    let result =
        parse_transcript_file(&fixture("agent_output_blocked_only.txt")).expect("read fixture");
    validate_trace_value(&result.trace).expect("trace validates");

    let last = result.trace.spans.last().expect("at least one span");
    assert_eq!(
        last.span_type,
        SpanType::WriteBlocked,
        "the denied write is the final span, not an abstain"
    );
    assert!(
        result
            .trace
            .spans
            .iter()
            .all(|s| s.span_type != SpanType::Abstain),
        "a blocked write must not also yield an abstain"
    );
    assert!(
        result
            .trace
            .spans
            .iter()
            .all(|s| s.span_type != SpanType::WriteAttempt),
        "the sole write was denied, so no write.attempt survives"
    );
}
