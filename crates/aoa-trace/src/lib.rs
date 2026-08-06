//! Trace-event substrate for the AOA Toolkit.
//!
//! Defines the OpenTelemetry-style span model emitted across the toolkit, the
//! trace-file format and its published JSON Schema, and a [`validate_trace`]
//! entrypoint that checks ordering and reports per-type span counts. Every
//! other crate in the workspace depends on these types.

mod envelope;
mod error;
mod metric_input;
mod model;
mod path_trust;
mod report;
mod span_type;
mod validate;

pub use envelope::{to_envelope_json_pretty, TraceEnvelope, TRACE_FORMAT_VERSION};
pub use error::TraceError;
pub use metric_input::{
    Confidence, IndexQuality, MetricInput, MetricInputRef, SymbolGraph, TransformMap,
};
pub use model::{Span, Trace};
#[cfg(unix)]
pub use path_trust::dirfd;
pub use path_trust::{
    is_symlink_nofollow, normalize_lexically, reject_symlink, resolve_canonicalizing,
    safe_join_nofollow, validate_single_component, PathTrustError, UnsafePathComponent,
};
pub use report::TraceReport;
pub use span_type::{SpanSource, SpanType};
pub use validate::{validate_trace, validate_trace_value};

/// The published JSON Schema for a trace file, embedded at compile time.
///
/// External producers and consumers validate trace files against this schema.
pub const TRACE_SCHEMA: &str = include_str!("../schema/trace.schema.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_schema_is_valid_json_describing_spans() {
        let schema: serde_json::Value =
            serde_json::from_str(TRACE_SCHEMA).expect("schema is valid JSON");
        assert!(schema.get("$schema").is_some());
        assert!(schema["properties"]["spans"].is_object());
    }

    #[test]
    fn embedded_schema_declares_optional_version() {
        let schema: serde_json::Value =
            serde_json::from_str(TRACE_SCHEMA).expect("schema is valid JSON");
        // The envelope's version must be described so external validators accept
        // versioned files (top-level additionalProperties is false).
        assert!(schema["properties"]["version"].is_object());
        assert_eq!(schema["properties"]["version"]["type"], "integer");
        // Version stays optional so legacy unversioned files remain schema-valid.
        let required = schema["required"].as_array().expect("required array");
        assert!(!required.iter().any(|r| r == "version"));
    }
}
