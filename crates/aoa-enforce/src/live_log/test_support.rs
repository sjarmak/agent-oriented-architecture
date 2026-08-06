//! Fixtures shared by the submodule test groups.
//!
//! They stage log *shapes* — a seeded file, a serialized line, a directory that
//! already exists — which more than one group needs and none of them owns.

use std::path::PathBuf;

use serde_json::{Map, Value};

use aoa_trace::{Span, SpanSource, SpanType};

/// A span of the given type and sequence, with the fields the lock tests
/// never vary. They assert on ordering and on whether a write landed at
/// all, so `source` and `attributes` are noise in every one of them.
pub(super) fn span(span_type: SpanType, seq: u64) -> Span {
    Span {
        span_type,
        source: SpanSource::Native,
        seq,
        attributes: Map::new(),
    }
}

/// A live-log path under `dir`, with its parent created.
pub(super) fn log_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    let log = dir.path().join(".aoa/traces").join(name);
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    log
}

/// Write `raw` as the whole log, bypassing the append path, so a test can
/// stage a log shape the append path would never produce.
pub(super) fn seed_log(dir: &tempfile::TempDir, raw: &str) -> PathBuf {
    let log = log_path(dir, "live-seeded.jsonl");
    std::fs::write(&log, raw).unwrap();
    log
}

/// One serialized span line, newline included.
pub(super) fn span_line(seq: u64) -> String {
    span_line_with(seq, Map::new())
}

/// [`span_line`], carrying `attributes` — which is how a fixture controls
/// the line's length.
pub(super) fn span_line_with(seq: u64, attributes: Map<String, Value>) -> String {
    let span = Span {
        span_type: SpanType::TestRun,
        source: SpanSource::Native,
        seq,
        attributes,
    };
    format!("{}\n", serde_json::to_string(&span).unwrap())
}
