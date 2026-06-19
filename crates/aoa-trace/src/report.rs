use std::collections::BTreeMap;

use crate::span_type::SpanType;

/// The result of validating a trace: per-type span counts plus a flag
/// indicating whether any span was reconstructed from logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceReport {
    counts: BTreeMap<SpanType, usize>,
    has_reconstructed: bool,
    total: usize,
}

impl TraceReport {
    pub(crate) fn new(counts: BTreeMap<SpanType, usize>, has_reconstructed: bool) -> Self {
        let total = counts.values().sum();
        Self {
            counts,
            has_reconstructed,
            total,
        }
    }

    /// Number of spans of a given type.
    pub fn count(&self, span_type: SpanType) -> usize {
        self.counts.get(&span_type).copied().unwrap_or(0)
    }

    /// Per-type counts. Only types that appeared at least once are present.
    pub fn counts(&self) -> &BTreeMap<SpanType, usize> {
        &self.counts
    }

    /// Total number of spans across all types.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Whether the trace contains at least one `reconstructed` span. Downstream
    /// consumers that require ground truth can use this to reject the trace.
    pub fn has_reconstructed(&self) -> bool {
        self.has_reconstructed
    }
}

impl PartialOrd for SpanType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SpanType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn index(t: &SpanType) -> usize {
            SpanType::ALL.iter().position(|x| x == t).unwrap()
        }
        index(self).cmp(&index(other))
    }
}
