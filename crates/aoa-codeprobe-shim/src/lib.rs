//! Trace shim: codeprobe stream-json transcript -> native 8-span AOA trace.
//!
//! codeprobe runs `claude -p --output-format stream-json --verbose` and writes
//! the (secret-sanitized) per-trial transcript to
//! `<runs_dir>/<task_id>/agent_output.txt`. Each line is one JSON event;
//! `assistant` events carry `tool_use` content blocks and `user` events carry
//! `tool_result` blocks. codeprobe's own reader only *counts* tool calls — this
//! shim instead preserves their **order** and **targets**, emitting an
//! [`aoa_trace::Trace`] of `source = native` spans. The strictly increasing
//! `seq` is what [`aoa_trace::validate_trace`] requires; the crate's integration
//! tests assert the emitted trace validates.
//!
//! # Tool -> span mapping
//!
//! | tool | span | target attribute |
//! |------|------|------------------|
//! | `Grep` / `Glob` / `*search*` | `retrieval.search` | `query` |
//! | `Read` | `file.read` | `path` |
//! | `Edit` / `Write` / `MultiEdit` / `NotebookEdit` | `write.attempt` | `path` |
//! | `Bash` running tests (`pytest`, `test.sh`, `cargo test`, …) | `test.run` | `command` |
//! | `mcp__*` | `gateway.invoke` | `tool` (the MCP tool name) |
//!
//! Plus two derived spans:
//! - a `write.attempt` whose `tool_result` reports `is_error: true` is
//!   reclassified to `write.blocked`;
//! - a transcript with no `write.attempt` at all gets a trailing `abstain` span.
//!
//! # Unmapped tools
//!
//! Tool names matching no rule above (including non-test `Bash`) are **not**
//! turned into a span — emitting an arbitrary default would corrupt the trace.
//! Instead each is recorded on [`ShimResult::warnings`] so it is logged and
//! never silently swallowed.
//!
//! # `symbol.lookup`
//!
//! This shim never emits `symbol.lookup`. That span is produced by joining tool
//! results against the SCIP graph (tracked separately as aoa-671), which may not
//! exist yet — so it is documented-absent here rather than fabricated.
//!
//! # Secrets
//!
//! This shim does **not** sanitize. Tool targets are lifted verbatim into span
//! attributes — the full `Bash` `command` and the `Read`/`Edit`/`Write` paths.
//! codeprobe strips secrets upstream when it writes `agent_output.txt`, so
//! callers MUST pass a codeprobe-sanitized transcript; feeding raw agent output
//! would carry any inline secret straight into the emitted trace. See
//! [`parse_transcript`] § Secrets.

mod error;
mod mapping;
mod parse;

pub use error::ShimError;
pub use parse::{parse_transcript, parse_transcript_file, ShimResult};
