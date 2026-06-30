//! Per-backend conformance tests for the cross-agent trace contract (R-silent).
//!
//! The same [`run_conformance`] harness validates every backend: the native
//! Claude reference and the reconstructed generic-log example. Each test carries
//! a freshness/version stamp — backends must declare the live [`CONTRACT_VERSION`]
//! or the harness rejects them.

use std::path::Path;

use aoa_codeprobe_shim::{
    run_conformance, ClaudeStreamJson, GenericLogBackend, TraceBackend, CONTRACT_VERSION,
};
use aoa_trace::{validate_trace_value, SpanSource};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("read fixture")
}

#[test]
fn claude_backend_conforms_as_the_native_reference() {
    let raw = read_fixture("agent_output.txt");
    let outcome = run_conformance(&ClaudeStreamJson, &raw).expect("claude backend conforms");

    assert_eq!(outcome.backend_id, "claude-stream-json");
    assert_eq!(outcome.contract_version, CONTRACT_VERSION);
    assert_eq!(outcome.declared_provenance, SpanSource::Native);
    assert!(
        !outcome.has_reconstructed,
        "the native reference emits no reconstructed spans"
    );
    // Grep, Read, mcp gateway, blocked Write, Edit, pytest Bash.
    assert_eq!(outcome.span_count, 6);
}

#[test]
fn generic_log_backend_conforms_as_a_reconstructed_example() {
    let raw = read_fixture("generic_agent_log.txt");
    let outcome = run_conformance(&GenericLogBackend, &raw).expect("generic backend conforms");

    assert_eq!(outcome.backend_id, "generic-log-reconstructed");
    assert_eq!(outcome.contract_version, CONTRACT_VERSION);
    assert_eq!(outcome.declared_provenance, SpanSource::Reconstructed);
    assert!(
        outcome.has_reconstructed,
        "reconstructed spans must be flagged so R7/R8 exclude them"
    );
    // search, read, gateway, blocked, write, test — the unmapped verb yields no span.
    assert_eq!(outcome.span_count, 6);
    assert_eq!(
        outcome.warnings, 1,
        "the unmapped verb is surfaced, not swallowed"
    );
}

#[test]
fn reconstructed_trace_validates_and_is_uniformly_reconstructed() {
    let raw = read_fixture("generic_agent_log.txt");
    let result = GenericLogBackend.parse(&raw).expect("parse generic log");

    validate_trace_value(&result.trace).expect("reconstructed trace validates");
    assert!(
        result
            .trace
            .spans
            .iter()
            .all(|s| s.source == SpanSource::Reconstructed),
        "every span the reconstructed backend emits is tagged reconstructed"
    );
}

#[test]
fn both_backends_declare_the_live_contract_version() {
    // Freshness stamp: the reference and the example are built against the
    // current contract revision, so the harness admits them.
    assert_eq!(ClaudeStreamJson.contract_version(), CONTRACT_VERSION);
    assert_eq!(GenericLogBackend.contract_version(), CONTRACT_VERSION);
}
