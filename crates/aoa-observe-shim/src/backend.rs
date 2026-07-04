//! The observe live-log [`TraceBackend`]: `.aoa/traces/live-<session>.jsonl`
//! (one [`Span`] per line, appended by the `aoa enforce` hooks) → a native
//! [`Trace`].

use aoa_codeprobe_shim::{ShimError, ShimResult, TraceBackend, CONTRACT_VERSION};
use aoa_trace::{Span, SpanSource, Trace};

/// The observe live-log ingestion backend.
///
/// Native provenance: the enforce hooks emit spans directly from the live
/// session (PostToolUse/PreToolUse), nothing is inferred after the fact. The
/// backend is validated against the cross-agent conformance contract by this
/// crate's tests, exactly like the codeprobe reference backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObserveLiveLog;

impl TraceBackend for ObserveLiveLog {
    fn backend_id(&self) -> &'static str {
        "observe-live-log"
    }

    fn contract_version(&self) -> &'static str {
        CONTRACT_VERSION
    }

    fn provenance(&self) -> SpanSource {
        SpanSource::Native
    }

    fn parse(&self, raw: &str) -> Result<ShimResult, ShimError> {
        parse_live_log(raw)
    }
}

/// Parse an observe live log (one span JSON object per line) into a trace.
///
/// The log format is owned by AOA's own enforce hooks, so a malformed line is
/// upstream corruption: it fails loud with [`ShimError::MalformedSpan`] naming
/// the 1-based line — never skipped, which would under-count the session and
/// corrupt span ordering. Blank lines are tolerated (a partial final append
/// leaves one). Warnings are always empty: this format has no lenient lane.
pub fn parse_live_log(raw: &str) -> Result<ShimResult, ShimError> {
    let spans: Vec<Span> = raw
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|source| ShimError::MalformedSpan {
                line: index + 1,
                source,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(ShimResult {
        trace: Trace { spans },
        warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_codeprobe_shim::run_conformance;

    const LOG: &str = concat!(
        r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#,
        "\n",
        r#"{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"src/lib.rs"}}"#,
        "\n",
    );

    #[test]
    fn parses_a_live_log_into_ordered_native_spans() {
        let result = parse_live_log(LOG).expect("well-formed log");
        assert_eq!(result.trace.spans.len(), 2);
        assert_eq!(result.trace.spans[0].seq, 0);
        assert_eq!(result.trace.spans[1].seq, 1);
        assert!(result.warnings.is_empty(), "no lenient lane");
        assert!(result
            .trace
            .spans
            .iter()
            .all(|s| s.source == SpanSource::Native));
    }

    #[test]
    fn blank_lines_are_tolerated() {
        let padded = format!("\n{LOG}\n\n");
        let result = parse_live_log(&padded).expect("blank lines tolerated");
        assert_eq!(result.trace.spans.len(), 2);
    }

    #[test]
    fn malformed_line_fails_loud_with_its_line_number() {
        let corrupt = format!("{LOG}not json\n");
        let err = parse_live_log(&corrupt).expect_err("corrupt line must fail");
        assert!(
            matches!(err, ShimError::MalformedSpan { line: 3, .. }),
            "expected MalformedSpan at line 3, got {err:?}"
        );
    }

    #[test]
    fn backend_conforms_to_the_cross_agent_contract() {
        let outcome = run_conformance(&ObserveLiveLog, LOG).expect("conformant");
        assert_eq!(outcome.backend_id, "observe-live-log");
        assert_eq!(outcome.declared_provenance, SpanSource::Native);
        assert_eq!(outcome.span_count, 2);
        assert!(!outcome.has_reconstructed);
        assert_eq!(outcome.warnings, 0);
    }
}
