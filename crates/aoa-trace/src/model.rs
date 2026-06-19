use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::span_type::{SpanSource, SpanType};

/// A single trace span.
///
/// Spans are ordered within a trace by their monotonic `seq` key. `attributes`
/// is a free-form object whose contents depend on the span type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    /// The kind of span; one of the eight stable discriminants.
    #[serde(rename = "type")]
    pub span_type: SpanType,
    /// Provenance: `native` or `reconstructed`.
    pub source: SpanSource,
    /// Monotonic ordering key within a trace.
    pub seq: u64,
    /// Free-form, span-type-specific metadata.
    #[serde(default)]
    pub attributes: Map<String, Value>,
}

/// A trace file: an ordered collection of spans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub spans: Vec<Span>,
}
