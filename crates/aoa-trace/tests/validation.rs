use std::path::PathBuf;

use aoa_trace::{
    to_envelope_json_pretty, validate_trace, Span, SpanSource, SpanType, Trace, TraceError,
    TRACE_FORMAT_VERSION, TRACE_SCHEMA,
};
use serde_json::{json, Value};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn schema_validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(TRACE_SCHEMA).expect("schema is valid JSON");
    jsonschema::draft202012::options()
        .build(&schema)
        .expect("schema compiles")
}

/// Peel the disk-boundary wrapper, asserting it carries `name`, then hand back
/// the inner failure so the caller can keep asserting the behavioural fields.
///
/// Only the structural path is checked; asserting the rendered `Display` here
/// would be tautological. The rendered chain a human reads is pinned by `aoa`'s
/// `validate_trace_{ordering,version}_error_names_the_file_once` CLI tests.
fn unwrap_invalid_file(err: TraceError, name: &str) -> TraceError {
    match err {
        TraceError::InvalidFile { path, source } => {
            assert!(
                path.ends_with(name),
                "boundary error must carry the offending path: {}",
                path.display()
            );
            *source
        }
        other => panic!("expected InvalidFile, got {other:?}"),
    }
}

#[test]
fn valid_trace_reports_per_type_counts() {
    let report = validate_trace(&fixture("valid.json")).expect("valid trace");

    assert_eq!(report.total(), 8);
    assert_eq!(report.count(SpanType::RetrievalSearch), 1);
    assert_eq!(report.count(SpanType::FileRead), 1);
    assert_eq!(report.count(SpanType::SymbolLookup), 1);
    assert_eq!(report.count(SpanType::WriteAttempt), 1);
    assert_eq!(report.count(SpanType::WriteBlocked), 1);
    assert_eq!(report.count(SpanType::TestRun), 1);
    assert_eq!(report.count(SpanType::GatewayInvoke), 1);
    assert_eq!(report.count(SpanType::Abstain), 1);
    assert!(!report.has_reconstructed());

    let summed: usize = report.counts().values().sum();
    assert_eq!(summed, report.total());
}

#[test]
fn out_of_order_trace_is_rejected() {
    let err = validate_trace(&fixture("out_of_order.json")).unwrap_err();
    match unwrap_invalid_file(err, "out_of_order.json") {
        TraceError::OutOfOrder {
            index,
            seq,
            previous,
        } => {
            assert_eq!(index, 2);
            assert_eq!(seq, 2);
            assert_eq!(previous, 5);
        }
        other => panic!("expected OutOfOrder, got {other:?}"),
    }
}

/// The ordering check itself stays path-free: a caller validating a `Trace` it
/// holds in memory has no file to name, so the path lives only at the disk
/// boundary. The version half of this guarantee is covered by
/// `envelope::tests::mismatched_version_is_rejected`, which matches the bare
/// `UnsupportedVersion` returned by `into_trace`.
#[test]
fn in_memory_ordering_failure_is_path_free() {
    let trace = Trace {
        spans: vec![
            Span {
                span_type: SpanType::TestRun,
                source: SpanSource::Native,
                seq: 5,
                attributes: serde_json::Map::new(),
            },
            Span {
                span_type: SpanType::TestRun,
                source: SpanSource::Native,
                seq: 2,
                attributes: serde_json::Map::new(),
            },
        ],
    };

    let err = aoa_trace::validate_trace_value(&trace).unwrap_err();
    assert!(
        matches!(err, TraceError::OutOfOrder { index: 1, .. }),
        "in-memory validation must not wrap in a path-carrying error: {err:?}"
    );
}

#[test]
fn schema_invalid_trace_is_rejected() {
    let err = validate_trace(&fixture("invalid_schema.json")).unwrap_err();
    assert!(
        matches!(err, TraceError::Schema { .. }),
        "expected Schema error, got {err:?}"
    );
}

#[test]
fn published_schema_rejects_a_file_read_without_path() {
    let value: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("missing_path_attribute.json")).expect("fixture reads"),
    )
    .expect("fixture is JSON");

    assert!(!schema_validator().is_valid(&value));
}

#[test]
fn published_schema_accepts_the_valid_trace_fixture() {
    let value: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("valid.json")).expect("fixture reads"),
    )
    .expect("fixture is JSON");

    assert!(schema_validator().is_valid(&value));
}

#[test]
fn published_schema_keeps_attributes_open_for_extension() {
    let value = json!({
        "spans": [{
            "type": "file.read",
            "source": "native",
            "seq": 0,
            "attributes": {
                "path": "src/lib.rs",
                "vendor.extension": {"enabled": true}
            }
        }]
    });

    assert!(schema_validator().is_valid(&value));
}

#[test]
fn published_schema_requires_metric_attributes_by_span_type() {
    let validator = schema_validator();
    for (span_type, key, value) in [
        ("retrieval.search", "query", "validate_trace"),
        ("file.read", "path", "src/lib.rs"),
        ("symbol.lookup", "symbol", "validate_trace"),
        ("write.attempt", "path", "src/lib.rs"),
        ("write.committed", "path", "src/lib.rs"),
        ("write.failed", "path", "src/lib.rs"),
        ("write.denied", "path", "src/lib.rs"),
        ("write.blocked", "policy", "reproduction_before_mutation"),
        ("test.run", "command", "cargo test"),
        ("gateway.invoke", "tool", "mcp__codegraph__search"),
    ] {
        let attributes: serde_json::Map<String, Value> =
            [(key.to_string(), Value::String(value.to_string()))]
                .into_iter()
                .collect();
        let valid = json!({
            "spans": [{
                "type": span_type,
                "source": "native",
                "seq": 0,
                "attributes": attributes
            }]
        });
        let missing = json!({
            "spans": [{"type": span_type, "source": "native", "seq": 0}]
        });
        assert!(validator.is_valid(&valid), "{span_type} with {key}");
        assert!(!validator.is_valid(&missing), "{span_type} without {key}");
    }
}

#[test]
fn published_schema_preserves_absent_results_as_no_evidence() {
    let retrieval_without_results = json!({
        "spans": [{
            "type": "retrieval.search",
            "source": "native",
            "seq": 0,
            "attributes": {"query": "validate_trace"}
        }]
    });
    assert!(schema_validator().is_valid(&retrieval_without_results));
}

#[test]
fn published_schema_requires_source_for_generated_artifact_blocks() {
    let validator = schema_validator();
    let generated_block = json!({
        "spans": [{
            "type": "write.blocked",
            "source": "native",
            "seq": 0,
            "attributes": {
                "policy": "generated_artifact",
                "source": "schema.json"
            }
        }]
    });
    let generated_block_without_source = json!({
        "spans": [{
            "type": "write.blocked",
            "source": "native",
            "seq": 0,
            "attributes": {"policy": "generated_artifact"}
        }]
    });
    assert!(validator.is_valid(&generated_block));
    assert!(!validator.is_valid(&generated_block_without_source));
}

#[test]
fn reconstructed_span_is_surfaced_and_round_trips() {
    let report = validate_trace(&fixture("reconstructed.json")).expect("valid trace");
    assert!(report.has_reconstructed());
    assert_eq!(report.total(), 3);

    let span = Span {
        span_type: SpanType::FileRead,
        source: SpanSource::Reconstructed,
        seq: 7,
        attributes: serde_json::Map::new(),
    };
    let json = serde_json::to_string(&span).expect("serialize span");
    let parsed: Span = serde_json::from_str(&json).expect("deserialize span");
    assert_eq!(parsed.source, SpanSource::Reconstructed);
    assert_eq!(parsed, span);
}

#[test]
fn missing_file_returns_read_error() {
    let err = validate_trace(&fixture("does_not_exist.json")).unwrap_err();
    assert!(
        matches!(err, TraceError::Read { .. }),
        "expected Read error, got {err:?}"
    );
}

/// The write-side envelope stamps a version, and a reader accepts what it wrote:
/// a full round-trip through the versioned wire format.
#[test]
fn versioned_trace_round_trips_through_disk() {
    let trace = Trace {
        spans: vec![
            Span {
                span_type: SpanType::RetrievalSearch,
                source: SpanSource::Native,
                seq: 0,
                attributes: serde_json::Map::new(),
            },
            Span {
                span_type: SpanType::TestRun,
                source: SpanSource::Native,
                seq: 1,
                attributes: serde_json::Map::new(),
            },
        ],
    };

    let json = to_envelope_json_pretty(&trace).expect("serialize versioned envelope");
    assert!(
        json.contains(&format!("\"version\": {TRACE_FORMAT_VERSION}")),
        "serialized trace must carry the wire-format version: {json}"
    );

    let dir = std::env::temp_dir().join(format!("aoa-trace-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("versioned.json");
    std::fs::write(&path, json).unwrap();

    let report = validate_trace(&path).expect("versioned trace validates");
    assert_eq!(report.total(), 2);
    assert_eq!(report.count(SpanType::RetrievalSearch), 1);
    assert_eq!(report.count(SpanType::TestRun), 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// A trace stamped with a version this build cannot parse is rejected fast,
/// rather than silently mis-parsed into the current span shape.
#[test]
fn mismatched_version_is_rejected() {
    let err = validate_trace(&fixture("bad_version.json")).unwrap_err();
    match unwrap_invalid_file(err, "bad_version.json") {
        TraceError::UnsupportedVersion { found, supported } => {
            assert_eq!(found, 999);
            assert_eq!(supported, TRACE_FORMAT_VERSION);
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

/// Traces written before versioning existed have no `version` key; they are
/// genuine current-format files and must still validate (non-breaking guard).
#[test]
fn unversioned_trace_still_validates() {
    let report = validate_trace(&fixture("valid.json")).expect("unversioned trace validates");
    assert_eq!(report.total(), 8);
}

/// A non-integer version is a structural parse failure, not a silent accept.
#[test]
fn malformed_version_is_a_schema_error() {
    let dir = std::env::temp_dir().join(format!("aoa-trace-badver-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.json");
    std::fs::write(&path, r#"{"version":"two","spans":[]}"#).unwrap();

    let err = validate_trace(&path).unwrap_err();
    assert!(
        matches!(err, TraceError::Schema { .. }),
        "expected Schema, got {err:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn equal_seq_is_allowed() {
    let trace = Trace {
        spans: vec![
            Span {
                span_type: SpanType::TestRun,
                source: SpanSource::Native,
                seq: 4,
                attributes: serde_json::Map::new(),
            },
            Span {
                span_type: SpanType::TestRun,
                source: SpanSource::Native,
                seq: 4,
                attributes: serde_json::Map::new(),
            },
        ],
    };
    let report = aoa_trace::validate_trace_value(&trace).expect("monotonic non-decreasing");
    assert_eq!(report.count(SpanType::TestRun), 2);
}
