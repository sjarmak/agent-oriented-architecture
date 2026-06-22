use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use crate::error::TraceError;
use crate::model::Trace;
use crate::report::TraceReport;
use crate::span_type::SpanSource;

/// Largest trace file read into memory. Trace files are small by nature; the cap
/// only trips pathological or hostile input so a crafted file cannot exhaust
/// memory before the schema parse.
const MAX_TRACE_BYTES: u64 = 16 * 1024 * 1024;

/// Read `path` into a `String`, rejecting anything past `max` bytes.
///
/// Bounded via [`Read::take`] rather than a pre-read `metadata().len()` check: a
/// file that grows (or a symlink whose target swaps) between stat and read cannot
/// blow past the cap. One byte past `max` is read so an exactly-`max` file is
/// accepted while a larger one is rejected.
fn read_capped(path: &Path, max: u64) -> Result<String, TraceError> {
    let file = std::fs::File::open(path).map_err(|source| TraceError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut raw = String::new();
    let read = file
        .take(max + 1)
        .read_to_string(&mut raw)
        .map_err(|source| TraceError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if read as u64 > max {
        return Err(TraceError::TooLarge {
            path: path.to_path_buf(),
            max,
        });
    }
    Ok(raw)
}

/// Load and validate a trace file at `path`.
///
/// Validation enforces:
/// - the file is schema-valid JSON (a `spans` array of well-typed spans), and
/// - spans are in monotonically non-decreasing `seq` order.
///
/// On success returns a [`TraceReport`] with per-span-type counts and a
/// `has_reconstructed` flag.
pub fn validate_trace(path: &Path) -> Result<TraceReport, TraceError> {
    let raw = read_capped(path, MAX_TRACE_BYTES)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_trace_file() {
        let dir = std::env::temp_dir().join(format!("aoa-trace-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trace.json");
        // One byte past the cap; content need not be valid JSON since the size
        // guard must trip before the schema parse is attempted.
        std::fs::write(&path, vec![b'x'; (MAX_TRACE_BYTES + 1) as usize]).unwrap();

        let err = validate_trace(&path).unwrap_err();
        assert!(
            matches!(err, TraceError::TooLarge { .. }),
            "expected TooLarge, got {err:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
