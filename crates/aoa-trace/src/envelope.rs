use serde::{Deserialize, Serialize};

use crate::error::TraceError;
use crate::model::{Span, Trace};

/// The trace wire-format version this build reads and writes.
///
/// This stamps the on-disk trace *envelope* (`{ "version": N, "spans": [...] }`)
/// so a producer-side format change fails fast on read instead of silently
/// mis-parsing. AOA ingests traces reconstructed from external codeprobe
/// transcripts; without a version a codeprobe-side change to the wire shape
/// would parse into subtly wrong spans with no error. A reader that finds a
/// version it does not support rejects the file (see [`check_format_version`]).
///
/// This is deliberately distinct from `aoa_codeprobe_shim::CONTRACT_VERSION`,
/// which versions the *backend conformance contract* (the `TraceBackend` shape),
/// not the serialized trace file. The two move independently.
pub const TRACE_FORMAT_VERSION: u32 = 1;

/// A file with no `version` key is treated as [`TRACE_FORMAT_VERSION`]: traces
/// written before versioning existed are genuine current-format files, so
/// accepting them keeps the guard non-breaking. The guard's job is to reject an
/// *explicit* mismatch (a producer that stamped a version this build cannot
/// parse), not to reject legacy files.
fn default_trace_format_version() -> u32 {
    TRACE_FORMAT_VERSION
}

/// The on-disk trace envelope: a format version plus the span collection.
///
/// Kept separate from the in-memory [`Trace`] so the version is purely a wire
/// concern — the domain model carries only spans. Every reader of a whole-trace
/// `.json` file deserializes into this type and then calls [`into_trace`], which
/// is the single choke point where the version is checked: there is no way to
/// obtain a [`Trace`] from an envelope without passing the version gate.
///
/// [`into_trace`]: TraceEnvelope::into_trace
///
/// `deny_unknown_fields` mirrors the published schema's top-level
/// `additionalProperties: false`: an unexpected envelope key is a malformed
/// trace, not something to silently ignore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceEnvelope {
    /// Wire-format version. Absent is treated as [`TRACE_FORMAT_VERSION`].
    #[serde(default = "default_trace_format_version")]
    pub version: u32,
    /// The ordered span collection.
    pub spans: Vec<Span>,
}

impl TraceEnvelope {
    /// Check the version and unwrap into the in-memory [`Trace`].
    ///
    /// Fails fast with [`TraceError::UnsupportedVersion`] if the declared
    /// version is one this build cannot parse. The error carries no path; every
    /// caller reading from disk wraps it with the offending file's path — see
    /// [`TraceError::InvalidFile`] — so the file is always named at the boundary.
    pub fn into_trace(self) -> Result<Trace, TraceError> {
        if self.version != TRACE_FORMAT_VERSION {
            return Err(TraceError::UnsupportedVersion {
                found: self.version,
                supported: TRACE_FORMAT_VERSION,
            });
        }
        Ok(Trace { spans: self.spans })
    }
}

/// The serialize-only view of an envelope. Borrows the spans so the sanctioned
/// write path does not deep-clone every span's `attributes` map just to
/// serialize once.
#[derive(Serialize)]
struct TraceEnvelopeRef<'a> {
    version: u32,
    spans: &'a [Span],
}

/// Serialize a trace to the versioned on-disk envelope as pretty JSON, stamping
/// the current [`TRACE_FORMAT_VERSION`].
///
/// This is the sanctioned way to write a whole-trace `.json` file: it guarantees
/// the version is present so the file survives a later read-side version check.
pub fn to_envelope_json_pretty(trace: &Trace) -> serde_json::Result<String> {
    let envelope = TraceEnvelopeRef {
        version: TRACE_FORMAT_VERSION,
        spans: &trace.spans,
    };
    serde_json::to_string_pretty(&envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span_type::{SpanSource, SpanType};

    fn one_span() -> Vec<Span> {
        vec![Span {
            span_type: SpanType::TestRun,
            source: SpanSource::Native,
            seq: 0,
            attributes: serde_json::Map::new(),
        }]
    }

    fn envelope(version: u32) -> TraceEnvelope {
        TraceEnvelope {
            version,
            spans: one_span(),
        }
    }

    #[test]
    fn mismatched_version_is_rejected() {
        let err = envelope(TRACE_FORMAT_VERSION + 1).into_trace().unwrap_err();
        match err {
            TraceError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, TRACE_FORMAT_VERSION + 1);
                assert_eq!(supported, TRACE_FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn envelope_round_trips_with_version() {
        let json = to_envelope_json_pretty(&Trace { spans: one_span() }).expect("serialize");
        assert!(
            json.contains("\"version\""),
            "envelope must carry a version: {json}"
        );

        let parsed: TraceEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.version, TRACE_FORMAT_VERSION);
        let trace = parsed.into_trace().expect("current version unwraps");
        assert_eq!(trace.spans, one_span());
    }

    #[test]
    fn missing_version_defaults_to_current() {
        let parsed: TraceEnvelope =
            serde_json::from_str(r#"{"spans":[]}"#).expect("deserialize unversioned");
        assert_eq!(parsed.version, TRACE_FORMAT_VERSION);
        assert!(parsed.into_trace().is_ok());
    }

    #[test]
    fn unknown_envelope_field_is_rejected() {
        let err = serde_json::from_str::<TraceEnvelope>(r#"{"spans":[],"bogus":1}"#).unwrap_err();
        assert!(
            err.to_string().contains("bogus") || err.to_string().contains("unknown"),
            "deny_unknown_fields should reject: {err}"
        );
    }

    #[test]
    fn malformed_version_type_is_a_parse_error() {
        // A non-integer version is a schema/parse failure, not a silent accept.
        assert!(serde_json::from_str::<TraceEnvelope>(r#"{"version":"two","spans":[]}"#).is_err());
        assert!(serde_json::from_str::<TraceEnvelope>(r#"{"version":null,"spans":[]}"#).is_err());
    }
}
