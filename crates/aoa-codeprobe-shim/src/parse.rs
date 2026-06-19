//! Walk a stream-json transcript into an ordered [`Trace`] of native spans.

use std::path::Path;

use aoa_trace::{Span, SpanSource, SpanType, Trace};
use serde_json::{Map, Value};

use crate::error::ShimError;
use crate::mapping::{classify, Mapping};

/// Outcome of parsing one transcript.
///
/// `trace` is always emittable (it passes `validate_trace`). `warnings` records
/// every non-fatal event the parser chose not to turn into a span — chiefly
/// unmapped tool names — so unknown tools are logged, never silently swallowed.
#[derive(Debug, Clone, PartialEq)]
pub struct ShimResult {
    pub trace: Trace,
    pub warnings: Vec<String>,
}

/// Parse a codeprobe `agent_output.txt` transcript at `path`.
///
/// Reads the file, then delegates to [`parse_transcript`].
pub fn parse_transcript_file(path: &Path) -> Result<ShimResult, ShimError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ShimError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(parse_transcript(&raw))
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
pub fn parse_transcript(raw: &str) -> ShimResult {
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
                warnings.push(format!("skipped non-JSON line: {}", truncate(line)));
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
                            warnings.push(format!("unmapped tool '{name}' (no span emitted)"));
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

    ShimResult {
        trace: Trace { spans },
        warnings,
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
