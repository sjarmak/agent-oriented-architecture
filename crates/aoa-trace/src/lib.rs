//! Trace-event substrate for the AOA Toolkit.
//!
//! Defines the OpenTelemetry-style span model emitted across the toolkit, the
//! trace-file format and its published JSON Schema, and a [`validate_trace`]
//! entrypoint that checks ordering and reports per-type span counts. Every
//! other crate in the workspace depends on these types.

mod error;
mod model;
mod report;
mod span_type;
mod validate;

pub use error::TraceError;
pub use model::{Span, Trace};
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
}
