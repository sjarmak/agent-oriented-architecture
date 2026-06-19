use serde::{Deserialize, Serialize};

use aoa_trace::{Span, SpanType};

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
/// attribute. Returns an empty vec when absent.
pub(crate) fn ranked_results(span: &Span) -> Vec<&str> {
    span.attributes
        .get("results")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default()
}
