use std::collections::BTreeMap;
use std::path::Path;

use crate::error::TraceError;
use crate::model::Trace;
use crate::report::TraceReport;
use crate::span_type::SpanSource;

/// Load and validate a trace file at `path`.
///
/// Validation enforces:
/// - the file is schema-valid JSON (a `spans` array of well-typed spans), and
/// - spans are in monotonically non-decreasing `seq` order.
///
/// On success returns a [`TraceReport`] with per-span-type counts and a
/// `has_reconstructed` flag.
pub fn validate_trace(path: &Path) -> Result<TraceReport, TraceError> {
    let raw = std::fs::read_to_string(path).map_err(|source| TraceError::Read {
        path: path.to_path_buf(),
        source,
    })?;

    let trace: Trace = serde_json::from_str(&raw).map_err(|source| TraceError::Schema {
        path: path.to_path_buf(),
        source,
    })?;

    validate_trace_value(&trace)
}

/// Validate an already-parsed [`Trace`], producing a report.
///
/// Separated from disk IO so callers holding a `Trace` in memory can reuse the
/// ordering checks and reporting.
pub fn validate_trace_value(trace: &Trace) -> Result<TraceReport, TraceError> {
    let mut counts: BTreeMap<crate::span_type::SpanType, usize> = BTreeMap::new();
    let mut has_reconstructed = false;
    let mut previous: Option<u64> = None;

    for (index, span) in trace.spans.iter().enumerate() {
        if let Some(prev) = previous {
            if span.seq < prev {
                return Err(TraceError::OutOfOrder {
                    index,
                    seq: span.seq,
                    previous: prev,
                });
            }
        }
        previous = Some(span.seq);

        *counts.entry(span.span_type).or_insert(0) += 1;
        if span.source == SpanSource::Reconstructed {
            has_reconstructed = true;
        }
    }

    Ok(TraceReport::new(counts, has_reconstructed))
}
