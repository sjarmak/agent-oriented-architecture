use serde::{Deserialize, Serialize};

use aoa_trace::{Span, SpanType};

use crate::error::MetricError;

/// The conditioning marker stamped on every metric record: all metrics are
/// reported conditioned on held-out success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionedOn {
    #[default]
    HeldOut,
}

/// Whether a span is a retrieval/read/lookup tool call that accesses an artifact.
pub(crate) fn is_access_span(span: &Span) -> bool {
    matches!(
        span.span_type,
        SpanType::RetrievalSearch | SpanType::FileRead | SpanType::SymbolLookup
    )
}

/// Whether a span reads an existing artifact (file.read or symbol.lookup).
/// Used for invariant discovery, which excludes search-only spans.
pub(crate) fn is_read_span(span: &Span) -> bool {
    matches!(span.span_type, SpanType::FileRead | SpanType::SymbolLookup)
}

/// The single artifact identifier a span touched, read from its `symbol` then
/// `path` attribute. Returns `None` for spans that name no artifact.
pub(crate) fn span_artifact(span: &Span) -> Option<&str> {
    span.attributes
        .get("symbol")
        .or_else(|| span.attributes.get("path"))
        .and_then(|v| v.as_str())
}

/// The ranked artifact list of a retrieval span, read from its `results`
/// attribute.
///
/// Absent and empty are distinct observations and the caller must be able to
/// tell them apart: `None` means the span carries no retrieval-ranking
/// instrumentation at all, while `Some(vec![])` means a retriever ran and
/// ranked nothing — a measured zero. Collapsing the two is what let a genuine
/// zero-recall retrieval report as absent evidence.
///
/// A present-but-malformed `results` is an error, not an empty list — a broken
/// measurement artifact must fail loudly rather than read as a measured zero.
pub(crate) fn ranked_results(span: &Span) -> Result<Option<Vec<&str>>, MetricError> {
    let Some(value) = span.attributes.get("results") else {
        return Ok(None);
    };
    let malformed = |found: String| MetricError::MalformedRankedResults { found };
    let array = value
        .as_array()
        .ok_or_else(|| malformed(format!("the attribute is {}", json_type_name(value))))?;
    array
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            entry
                .as_str()
                .ok_or_else(|| malformed(format!("entry {i} is {}", json_type_name(entry))))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The JSON type name of a value, for naming what a malformed attribute held.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}
