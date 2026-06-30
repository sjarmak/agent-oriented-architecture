//! The versioned cross-agent trace conformance contract (R-silent).
//!
//! AOA scores agent runs by reconstructing the 8-span [`aoa_trace::Trace`] from a
//! transcript. The Claude stream-json shim is one such reconstruction; other
//! agents (Copilot, Codex) emit differently shaped logs. This module defines the
//! contract every agent-log adapter implements so the toolkit can score any of
//! them through one validated instrument rather than a per-agent special case.
//!
//! A [`TraceBackend`] maps a raw transcript to a trace and **declares its
//! provenance posture**: a `native` backend reads spans an instrumented agent
//! emitted directly; a `reconstructed` backend infers them from heterogeneous
//! logs. Provenance is a first-class, per-span field ([`aoa_trace::SpanSource`])
//! so downstream R7/R8 metrics can exclude reconstructed spans
//! ([`aoa_trace::TraceReport::has_reconstructed`]).
//!
//! [`run_conformance`] is the single harness that validates *any* backend:
//! freshness gate (the backend targets the live [`CONTRACT_VERSION`]), structural
//! validity (the trace passes [`aoa_trace::validate_trace_value`]), and provenance
//! agreement (every span carries the declared posture).
//!
//! # Versioning
//!
//! [`CONTRACT_VERSION`] governs the *backend* contract — the trait shape and the
//! conformance invariants — and is distinct from the trace *wire* schema
//! ([`aoa_trace::TRACE_SCHEMA`]). A backend stamps the contract revision it was
//! built against via [`TraceBackend::contract_version`]; the harness rejects any
//! backend whose stamp has drifted from the live revision.

use aoa_trace::{validate_trace_value, SpanSource};

use crate::error::ShimError;
use crate::parse::{parse_transcript, ShimResult};

/// Version of the cross-agent trace conformance contract.
///
/// Bumped when the [`TraceBackend`] shape or the conformance invariants change.
/// Backends declare the revision they target; [`run_conformance`] gates on it.
pub const CONTRACT_VERSION: &str = "1.0.0";

/// A backend that maps one agent's transcript into an [`aoa_trace::Trace`].
///
/// Implementors declare a stable [`backend_id`](TraceBackend::backend_id), the
/// [`contract_version`](TraceBackend::contract_version) they target, and their
/// [`provenance`](TraceBackend::provenance) posture, then map a raw transcript in
/// [`parse`](TraceBackend::parse). The reference implementation is
/// [`ClaudeStreamJson`] (native); [`crate::GenericLogBackend`] is a reconstructed
/// example proving the contract generalizes beyond Claude.
pub trait TraceBackend {
    /// Stable identifier for the agent/log format this backend adapts.
    fn backend_id(&self) -> &'static str;

    /// The contract revision this backend was built against. Checked against the
    /// live [`CONTRACT_VERSION`] by [`run_conformance`].
    fn contract_version(&self) -> &'static str;

    /// Whether spans are read from a natively instrumented agent (`native`) or
    /// inferred from heterogeneous logs (`reconstructed`). Every span the backend
    /// emits must carry this posture.
    fn provenance(&self) -> SpanSource;

    /// Map a raw transcript into a trace, tagging every span with
    /// [`provenance`](TraceBackend::provenance).
    fn parse(&self, raw: &str) -> Result<ShimResult, ShimError>;
}

/// The reference backend: codeprobe's Claude `--output-format stream-json`
/// transcript, read as native spans. Wraps [`parse_transcript`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeStreamJson;

impl TraceBackend for ClaudeStreamJson {
    fn backend_id(&self) -> &'static str {
        "claude-stream-json"
    }

    fn contract_version(&self) -> &'static str {
        CONTRACT_VERSION
    }

    fn provenance(&self) -> SpanSource {
        SpanSource::Native
    }

    fn parse(&self, raw: &str) -> Result<ShimResult, ShimError> {
        parse_transcript(raw)
    }
}

/// A stamped result of running a backend through [`run_conformance`].
///
/// Carries the contract revision the backend was validated against (the
/// freshness stamp) alongside the shape of the trace it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceOutcome {
    /// The backend that was validated.
    pub backend_id: &'static str,
    /// The contract revision the backend was validated against.
    pub contract_version: &'static str,
    /// The provenance posture the backend declared and every span carried.
    pub declared_provenance: SpanSource,
    /// Number of spans in the produced trace.
    pub span_count: usize,
    /// Whether the trace contains reconstructed spans (excluded from R7/R8).
    pub has_reconstructed: bool,
    /// Number of non-fatal warnings the backend surfaced (e.g. unmapped tools).
    pub warnings: usize,
}

/// Validate a backend against the conformance contract over `transcript`.
///
/// Three gates, all fail-loud:
/// 1. **Freshness** — the backend must target the live [`CONTRACT_VERSION`].
/// 2. **Structural validity** — the produced trace must pass
///    [`validate_trace_value`] (ordered `seq`, well-typed spans).
/// 3. **Provenance agreement** — every span must carry the backend's declared
///    posture, so a `native` backend cannot smuggle in `reconstructed` spans.
///
/// Returns a stamped [`ConformanceOutcome`] on success.
pub fn run_conformance(
    backend: &dyn TraceBackend,
    transcript: &str,
) -> Result<ConformanceOutcome, ShimError> {
    if backend.contract_version() != CONTRACT_VERSION {
        return Err(ShimError::ContractVersionMismatch {
            backend_id: backend.backend_id(),
            declared: backend.contract_version(),
            expected: CONTRACT_VERSION,
        });
    }

    let result = backend.parse(transcript)?;
    validate_trace_value(&result.trace)?;

    let declared = backend.provenance();
    for (index, span) in result.trace.spans.iter().enumerate() {
        if span.source != declared {
            return Err(ShimError::ProvenanceMismatch {
                backend_id: backend.backend_id(),
                index,
                declared,
                found: span.source,
            });
        }
    }

    Ok(ConformanceOutcome {
        backend_id: backend.backend_id(),
        contract_version: CONTRACT_VERSION,
        declared_provenance: declared,
        span_count: result.trace.spans.len(),
        has_reconstructed: declared == SpanSource::Reconstructed,
        warnings: result.warnings.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_trace::{Span, SpanType, Trace};
    use serde_json::Map;

    /// A backend stamped with a stale contract version, used to exercise the
    /// freshness gate.
    struct StaleBackend;
    impl TraceBackend for StaleBackend {
        fn backend_id(&self) -> &'static str {
            "stale"
        }
        fn contract_version(&self) -> &'static str {
            "0.9.0"
        }
        fn provenance(&self) -> SpanSource {
            SpanSource::Native
        }
        fn parse(&self, _raw: &str) -> Result<ShimResult, ShimError> {
            Ok(ShimResult {
                trace: Trace { spans: vec![] },
                warnings: vec![],
            })
        }
    }

    /// A backend that declares `native` but emits a `reconstructed` span, used to
    /// exercise the provenance gate.
    struct LyingBackend;
    impl TraceBackend for LyingBackend {
        fn backend_id(&self) -> &'static str {
            "lying"
        }
        fn contract_version(&self) -> &'static str {
            CONTRACT_VERSION
        }
        fn provenance(&self) -> SpanSource {
            SpanSource::Native
        }
        fn parse(&self, _raw: &str) -> Result<ShimResult, ShimError> {
            Ok(ShimResult {
                trace: Trace {
                    spans: vec![Span {
                        span_type: SpanType::FileRead,
                        source: SpanSource::Reconstructed,
                        seq: 0,
                        attributes: Map::new(),
                    }],
                },
                warnings: vec![],
            })
        }
    }

    #[test]
    fn freshness_gate_rejects_a_stale_backend() {
        let err = run_conformance(&StaleBackend, "").unwrap_err();
        assert!(matches!(
            err,
            ShimError::ContractVersionMismatch {
                declared: "0.9.0",
                expected: CONTRACT_VERSION,
                ..
            }
        ));
    }

    #[test]
    fn provenance_gate_rejects_a_mismatched_span() {
        let err = run_conformance(&LyingBackend, "").unwrap_err();
        assert!(matches!(
            err,
            ShimError::ProvenanceMismatch {
                index: 0,
                declared: SpanSource::Native,
                found: SpanSource::Reconstructed,
                ..
            }
        ));
    }

    #[test]
    fn claude_backend_is_the_native_reference() {
        assert_eq!(ClaudeStreamJson.backend_id(), "claude-stream-json");
        assert_eq!(ClaudeStreamJson.provenance(), SpanSource::Native);
        assert_eq!(ClaudeStreamJson.contract_version(), CONTRACT_VERSION);
    }
}
