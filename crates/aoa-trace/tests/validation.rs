use std::path::PathBuf;

use aoa_trace::{validate_trace, Span, SpanSource, SpanType, Trace, TraceError};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
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
    match err {
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

#[test]
fn schema_invalid_trace_is_rejected() {
    let err = validate_trace(&fixture("invalid_schema.json")).unwrap_err();
    assert!(
        matches!(err, TraceError::Schema { .. }),
        "expected Schema error, got {err:?}"
    );
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
