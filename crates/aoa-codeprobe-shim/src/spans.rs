use std::collections::HashMap;

use aoa_trace::{Span, SpanSource, SpanType, Trace};
use serde_json::{Map, Value};

use crate::error::ShimError;
use crate::mapping::{classify, Mapping};

/// Largest span count a single transcript may produce. Well above any real run
/// (a 64 MiB transcript of minimal tool_use blocks tops out near ~1.3M spans);
/// hitting this means the input is pathological and parsing fails loud.
pub(crate) const MAX_SPANS: usize = 200_000;

/// Largest number of warnings retained. Warnings are lossy diagnostics, so the
/// cap drops extras behind a sentinel rather than erroring — this bounds the
/// amplification of a file made entirely of tiny non-JSON lines.
const MAX_WARNINGS: usize = 10_000;

/// Resource bounds applied while parsing, factored out so tests can exercise the
/// caps with tiny values instead of materializing a multi-MiB transcript.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_spans: usize,
    pub(crate) max_warnings: usize,
}

impl Limits {
    pub(crate) const DEFAULT: Self = Self {
        max_spans: MAX_SPANS,
        max_warnings: MAX_WARNINGS,
    };
}

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

pub(crate) struct SpanBuilder {
    spans: Vec<Span>,
    warnings: Vec<String>,
    span_index_by_tool_id: HashMap<String, usize>,
    saw_write: bool,
    limits: Limits,
}

impl SpanBuilder {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            spans: Vec::new(),
            warnings: Vec::new(),
            span_index_by_tool_id: HashMap::new(),
            saw_write: false,
            limits,
        }
    }

    pub(crate) fn tool_use(
        &mut self,
        id: Option<&str>,
        name: &str,
        input: &Value,
    ) -> Result<(), ShimError> {
        match classify(name, input) {
            Mapping::Span {
                span_type,
                target_key,
                target_fields,
            } => {
                let mut attributes = Map::new();
                if let Some(target) = resolve_target(name, target_fields, input) {
                    attributes.insert(target_key.to_string(), Value::String(target));
                }
                attributes.insert("tool".to_string(), Value::String(name.to_string()));

                if span_type == SpanType::WriteAttempt {
                    self.saw_write = true;
                }
                if self.spans.len() >= self.limits.max_spans {
                    return Err(ShimError::TooManySpans {
                        max: self.limits.max_spans,
                    });
                }
                if let Some(id) = id {
                    self.span_index_by_tool_id
                        .insert(id.to_string(), self.spans.len());
                }
                self.spans.push(Span {
                    span_type,
                    source: SpanSource::Native,
                    seq: self.spans.len() as u64,
                    attributes,
                });
            }
            Mapping::Unknown => {
                self.warn(format!("unmapped tool '{name}' (no span emitted)"));
            }
        }
        Ok(())
    }

    pub(crate) fn tool_result(&mut self, tool_use_id: &str, is_error: bool) {
        let Some(&idx) = self.span_index_by_tool_id.get(tool_use_id) else {
            return;
        };
        if self.spans[idx].span_type != SpanType::WriteAttempt {
            return;
        }
        self.spans[idx].span_type = if is_error {
            SpanType::WriteBlocked
        } else {
            SpanType::WriteCommitted
        };
    }

    pub(crate) fn warn(&mut self, msg: String) {
        let max = self.limits.max_warnings;
        if self.warnings.len() < max {
            self.warnings.push(msg);
        } else if self.warnings.len() == max {
            self.warnings.push(format!(
                "warning cap reached: further warnings suppressed (>{max})"
            ));
        }
    }

    pub(crate) fn finish(mut self) -> ShimResult {
        if !self.saw_write {
            self.spans.push(Span {
                span_type: SpanType::Abstain,
                source: SpanSource::Native,
                seq: self.spans.len() as u64,
                attributes: Map::new(),
            });
        }
        ShimResult {
            trace: Trace { spans: self.spans },
            warnings: self.warnings,
        }
    }
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
