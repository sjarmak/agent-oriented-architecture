//! Walk a stream-json transcript into an ordered [`Trace`] of native spans.

use std::io::Read;
use std::path::Path;

use aoa_trace::{Span, SpanSource, SpanType, Trace};
use serde_json::{Map, Value};

use crate::error::ShimError;
use crate::mapping::{classify, Mapping};

/// Largest transcript accepted from disk. A long verbose agent run is a few tens
/// of MiB; this leaves generous headroom while bounding the bytes held in memory
/// from an attacker-controlled file.
const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;

/// Largest span count a single transcript may produce. Well above any real run
/// (a 64 MiB transcript of minimal tool_use blocks tops out near ~1.3M spans);
/// hitting this means the input is pathological and parsing fails loud.
const MAX_SPANS: usize = 200_000;

/// Largest number of warnings retained. Warnings are lossy diagnostics, so the
/// cap drops extras behind a sentinel rather than erroring — this bounds the
/// amplification of a file made entirely of tiny non-JSON lines.
const MAX_WARNINGS: usize = 10_000;

/// Outcome of parsing one transcript.
///
/// `trace` is built with a strictly increasing `seq`, the invariant
/// `validate_trace` checks (asserted by the crate's integration tests).
/// `warnings` records every non-fatal event the parser chose not to turn into a
/// span — chiefly unmapped tool names — so unknown tools are logged, never
/// silently swallowed.
#[derive(Debug, Clone, PartialEq)]
pub struct ShimResult {
    pub trace: Trace,
    pub warnings: Vec<String>,
}

/// Parse a codeprobe `agent_output.txt` transcript at `path`.
///
/// Reads the file under a byte cap, then delegates to [`parse_transcript`]. The
/// read is bounded via [`Read::take`] rather than a pre-read `metadata().len()`
/// check so a file that grows (or a symlink whose target swaps) between stat and
/// read cannot blow past the cap — the threat model is attacker-controlled local
/// files.
pub fn parse_transcript_file(path: &Path) -> Result<ShimResult, ShimError> {
    let raw = read_capped(path, MAX_TRANSCRIPT_BYTES)?;
    parse_transcript(&raw)
}

/// Read a file into a `String`, rejecting anything past `max` bytes.
///
/// Bounded via [`Read::take`] rather than a pre-read `metadata().len()` check: a
/// file that grows (or a symlink whose target swaps) between stat and read cannot
/// blow past the cap. One extra byte is read so an exactly-`max` file is accepted
/// while a larger one is rejected.
pub(crate) fn read_capped(path: &Path, max: u64) -> Result<String, ShimError> {
    let file = std::fs::File::open(path).map_err(|source| ShimError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut raw = String::new();
    let read = file
        .take(max + 1)
        .read_to_string(&mut raw)
        .map_err(|source| ShimError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if read as u64 > max {
        return Err(ShimError::TranscriptTooLarge {
            path: path.to_path_buf(),
            max,
        });
    }
    Ok(raw)
}

/// Parse a newline-delimited stream-json transcript into a native trace.
///
/// Each line is one JSON event (`claude -p --output-format stream-json
/// --verbose`). `assistant` events carry `tool_use` content blocks, which are
/// mapped to spans in transcript order with a strictly increasing `seq`. `user`
/// events carry `tool_result` blocks; an errored result on a `write.attempt`
/// span reclassifies it to `write.blocked`. If no write was attempted across
/// the whole transcript, a trailing `abstain` span is appended.
///
/// Blank and non-JSON lines are skipped (matching codeprobe's reader). All
/// emitted spans have `source = native`.
///
/// Returns [`ShimError::TooManySpans`] if the transcript would produce more than
/// [`MAX_SPANS`] spans: a silently truncated trace would corrupt the locality
/// metrics computed from it, so the bound fails loud. Warnings, being lossy
/// diagnostics, are capped behind a sentinel instead (see [`MAX_WARNINGS`]).
pub fn parse_transcript(raw: &str) -> Result<ShimResult, ShimError> {
    parse_transcript_bounded(raw, Limits::DEFAULT)
}

/// Resource bounds applied while parsing, factored out so tests can exercise the
/// caps with tiny values instead of materializing a multi-MiB transcript.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    max_spans: usize,
    max_warnings: usize,
}

impl Limits {
    const DEFAULT: Self = Self {
        max_spans: MAX_SPANS,
        max_warnings: MAX_WARNINGS,
    };
}

pub(crate) fn parse_transcript_bounded(raw: &str, limits: Limits) -> Result<ShimResult, ShimError> {
    let mut spans: Vec<Span> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    // Maps a tool_use id to the index of the span it produced, so a later
    // tool_result can reclassify it (e.g. write.attempt -> write.blocked).
    let mut span_index_by_tool_id: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut seq: u64 = 0;
    let mut saw_write = false;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                record_warning(
                    &mut warnings,
                    limits.max_warnings,
                    format!("skipped non-JSON line: {}", truncate(line)),
                );
                continue;
            }
        };

        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                for block in content_blocks(&event) {
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    let empty = Value::Object(Map::new());
                    let input = block.get("input").unwrap_or(&empty);

                    match classify(name, input) {
                        Mapping::Span {
                            span_type,
                            target_key,
                            target_fields,
                        } => {
                            let target = resolve_target(name, target_fields, input);
                            let mut attributes = Map::new();
                            if let Some(t) = target {
                                attributes.insert(target_key.to_string(), Value::String(t));
                            }
                            attributes.insert("tool".to_string(), Value::String(name.to_string()));

                            if span_type == SpanType::WriteAttempt {
                                saw_write = true;
                            }
                            if spans.len() >= limits.max_spans {
                                return Err(ShimError::TooManySpans {
                                    max: limits.max_spans,
                                });
                            }
                            if let Some(id) = block.get("id").and_then(Value::as_str) {
                                span_index_by_tool_id.insert(id.to_string(), spans.len());
                            }
                            spans.push(Span {
                                span_type,
                                source: SpanSource::Native,
                                seq,
                                attributes,
                            });
                            seq += 1;
                        }
                        Mapping::Unknown => {
                            record_warning(
                                &mut warnings,
                                limits.max_warnings,
                                format!("unmapped tool '{name}' (no span emitted)"),
                            );
                        }
                    }
                }
            }
            Some("user") => {
                for block in content_blocks(&event) {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    if !block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let id = match block.get("tool_use_id").and_then(Value::as_str) {
                        Some(id) => id,
                        None => continue,
                    };
                    if let Some(&idx) = span_index_by_tool_id.get(id) {
                        if spans[idx].span_type == SpanType::WriteAttempt {
                            spans[idx].span_type = SpanType::WriteBlocked;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if !saw_write {
        spans.push(Span {
            span_type: SpanType::Abstain,
            source: SpanSource::Native,
            seq,
            attributes: Map::new(),
        });
    }

    Ok(ShimResult {
        trace: Trace { spans },
        warnings,
    })
}

/// Append `msg` to `warnings`, capping growth at `max`. The entry that reaches
/// the cap becomes a sentinel so the truncation is visible, never silent.
fn record_warning(warnings: &mut Vec<String>, max: usize, msg: String) {
    if warnings.len() < max {
        warnings.push(msg);
    } else if warnings.len() == max {
        warnings.push(format!(
            "warning cap reached: further warnings suppressed (>{max})"
        ));
    }
}

/// Pull the `content` array from an event's `message` object.
fn content_blocks(event: &Value) -> impl Iterator<Item = &Value> {
    event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
}

/// Resolve a span's target string.
///
/// MCP tools have no input field for their target; the tool name itself is the
/// meaningful target, so it is used directly. All other tools read the first
/// present of `fields` from the tool `input`.
fn resolve_target(name: &str, fields: &[&str], input: &Value) -> Option<String> {
    if name.starts_with("mcp__") {
        return Some(name.to_string());
    }
    fields
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
}

fn truncate(s: &str) -> String {
    const MAX: usize = 80;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let prefix: String = s.chars().take(MAX).collect();
        format!("{prefix}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `assistant` event carrying a single `Read` tool_use → one span.
    fn read_event() -> String {
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"/x"}}]}}"#.to_string()
    }

    fn limits(max_spans: usize, max_warnings: usize) -> Limits {
        Limits {
            max_spans,
            max_warnings,
        }
    }

    #[test]
    fn read_capped_rejects_files_over_the_cap() {
        let dir = std::env::temp_dir().join(format!("aoa-shim-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.txt");
        std::fs::write(&path, "0123456789").unwrap(); // 10 bytes

        // A cap below the file size is rejected as TranscriptTooLarge...
        let err = read_capped(&path, 4).unwrap_err();
        assert!(matches!(err, ShimError::TranscriptTooLarge { max: 4, .. }));
        // ...while a cap at exactly the file size is accepted.
        assert_eq!(read_capped(&path, 10).unwrap().len(), 10);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn span_cap_fails_loud_rather_than_truncating() {
        // Three spannable events with a span cap of 2 must error, not return a
        // truncated 2-span trace (which would corrupt locality metrics).
        let raw = format!("{0}\n{0}\n{0}\n", read_event());
        let err = parse_transcript_bounded(&raw, limits(2, 1_000)).unwrap_err();
        assert!(matches!(err, ShimError::TooManySpans { max: 2 }));

        // The same input under a sufficient cap parses cleanly: three reads plus
        // the trailing `abstain` span (no write was attempted).
        let ok = parse_transcript_bounded(&raw, limits(8, 1_000)).unwrap();
        assert_eq!(ok.trace.spans.len(), 4);
    }

    #[test]
    fn warning_cap_truncates_behind_a_visible_sentinel() {
        // Five non-JSON lines with a warning cap of 2: two real warnings plus one
        // sentinel, and the sentinel names the cap so the truncation is visible.
        let raw = "x\ny\nz\nw\nv\n";
        let result = parse_transcript_bounded(raw, limits(100, 2)).unwrap();
        assert_eq!(result.warnings.len(), 3);
        assert!(result
            .warnings
            .last()
            .unwrap()
            .contains("further warnings suppressed"));
    }
}
