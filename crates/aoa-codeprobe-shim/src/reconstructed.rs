//! A reconstructed-provenance example backend, proving the [`TraceBackend`]
//! contract generalizes beyond Claude stream-json.
//!
//! [`GenericLogBackend`] adapts a deliberately minimal, agent-agnostic log
//! format — one `<verb> <target>` per line — into a trace whose spans are all
//! tagged [`SpanSource::Reconstructed`]. It exists to demonstrate that a second,
//! differently shaped transcript maps cleanly through the same contract and the
//! same [`crate::run_conformance`] harness.
//!
//! This is **not** a production Copilot/Codex adapter and makes no claim about a
//! real second-agent result: per the bead's P3 gating, no live R0 verdict exists
//! yet, so widening to a real second agent would validate an uncalibrated
//! instrument. The format here is illustrative and driven by checked-in
//! fixtures. Because every span is reconstructed, the existing
//! [`aoa_trace::TraceReport::has_reconstructed`] flag excludes this backend's
//! traces from R7/R8 ground-truth metrics.
//!
//! # Format
//!
//! Blank lines and `#` comment lines are ignored. Each remaining line is a verb
//! and a target separated by whitespace:
//!
//! | verb | span | target attribute |
//! |------|------|------------------|
//! | `search` | `retrieval.search` | `query` |
//! | `read` | `file.read` | `path` |
//! | `write` | `write.attempt` | `path` |
//! | `blocked` | `write.blocked` | `path` |
//! | `test` | `test.run` | `command` |
//! | `gateway` | `gateway.invoke` | `tool` |
//!
//! An unmapped verb is surfaced on [`ShimResult::warnings`], never silently
//! dropped — mirroring the Claude shim's treatment of unmapped tools.

use aoa_trace::{Span, SpanSource, SpanType, Trace};
use serde_json::{Map, Value};

use crate::backend::{TraceBackend, CONTRACT_VERSION};
use crate::error::ShimError;
use crate::parse::ShimResult;

/// The reconstructed-provenance example backend (see module docs).
#[derive(Debug, Clone, Copy, Default)]
pub struct GenericLogBackend;

impl TraceBackend for GenericLogBackend {
    fn backend_id(&self) -> &'static str {
        "generic-log-reconstructed"
    }

    fn contract_version(&self) -> &'static str {
        CONTRACT_VERSION
    }

    fn provenance(&self) -> SpanSource {
        SpanSource::Reconstructed
    }

    fn parse(&self, raw: &str) -> Result<ShimResult, ShimError> {
        parse_generic_log(raw)
    }
}

/// Map a log verb to its span type and the attribute key its target is stored
/// under. Returns `None` for an unmapped verb.
fn map_verb(verb: &str) -> Option<(SpanType, &'static str)> {
    match verb {
        "search" => Some((SpanType::RetrievalSearch, "query")),
        "read" => Some((SpanType::FileRead, "path")),
        "write" => Some((SpanType::WriteAttempt, "path")),
        "blocked" => Some((SpanType::WriteBlocked, "path")),
        "test" => Some((SpanType::TestRun, "command")),
        "gateway" => Some((SpanType::GatewayInvoke, "tool")),
        _ => None,
    }
}

/// Parse the generic `<verb> <target>` log into a reconstructed trace.
pub(crate) fn parse_generic_log(raw: &str) -> Result<ShimResult, ShimError> {
    let mut spans: Vec<Span> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut seq: u64 = 0;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (verb, target) = match line.split_once(char::is_whitespace) {
            Some((verb, target)) => (verb, target.trim()),
            None => (line, ""),
        };

        match map_verb(verb) {
            Some((span_type, target_key)) => {
                let mut attributes = Map::new();
                if !target.is_empty() {
                    attributes.insert(target_key.to_string(), Value::String(target.to_string()));
                }
                spans.push(Span {
                    span_type,
                    source: SpanSource::Reconstructed,
                    seq,
                    attributes,
                });
                seq += 1;
            }
            None => warnings.push(format!("unmapped log verb '{verb}' (no span emitted)")),
        }
    }

    Ok(ShimResult {
        trace: Trace { spans },
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_verb_to_its_span_in_order() {
        let log = "search q\nread p\ngateway t\nblocked b\nwrite w\ntest c\n";
        let result = parse_generic_log(log).unwrap();
        let types: Vec<SpanType> = result.trace.spans.iter().map(|s| s.span_type).collect();
        assert_eq!(
            types,
            vec![
                SpanType::RetrievalSearch,
                SpanType::FileRead,
                SpanType::GatewayInvoke,
                SpanType::WriteBlocked,
                SpanType::WriteAttempt,
                SpanType::TestRun,
            ]
        );
        assert!(result.warnings.is_empty());
        // seq is strictly increasing.
        let seqs: Vec<u64> = result.trace.spans.iter().map(|s| s.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn unmapped_verb_is_warned_not_swallowed() {
        let result = parse_generic_log("frobnicate x\nread p\n").unwrap();
        assert_eq!(result.trace.spans.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("frobnicate"));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let result = parse_generic_log("# header\n\nread p\n").unwrap();
        assert_eq!(result.trace.spans.len(), 1);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn every_span_is_tagged_reconstructed() {
        let result = parse_generic_log("search q\nread p\n").unwrap();
        assert!(result
            .trace
            .spans
            .iter()
            .all(|s| s.source == SpanSource::Reconstructed));
    }
}
