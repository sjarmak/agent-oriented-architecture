//! Trace shim: a codeprobe trial -> native 8-span AOA trace.
//!
//! codeprobe runs `claude -p --output-format stream-json --verbose` once per
//! trial and leaves two records of what the agent did:
//!
//! - `<runs_dir>/<config>/<task_id>/agent_output.txt`, the per-trial transcript.
//!   Older codeprobe wrote the whole (secret-sanitized) stream-json event stream
//!   here; current codeprobe writes only the agent's extracted final answer.
//! - `<runs_dir>/trace.db`, one SQLite database for the whole run into which
//!   codeprobe's telemetry adapter ingests every trial's `tool_use` events.
//!
//! [`parse_trial`] reads the transcript when it carries agent events and the
//! database otherwise; [`parse_transcript`] and [`parse_trace_db`] are the two
//! sources on their own. Either way the shim preserves the tool calls'
//! **order** and **targets** (codeprobe's own reader only counts them),
//! emitting an [`aoa_trace::Trace`] of `source = native` spans through one
//! shared builder, so a tool call maps to the same span whichever file it was
//! read from. The strictly increasing `seq` is what
//! [`aoa_trace::validate_trace`] requires; the crate's integration tests assert
//! the emitted trace validates.
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
//!   reclassified to `write.blocked`, and one whose result succeeded to
//!   `write.committed`. Only the transcript carries `tool_result` blocks;
//!   `trace.db` records none, so a trial read from it keeps every write at
//!   `write.attempt`;
//! - a trial with no `write.attempt` at all gets a trailing `abstain` span.
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
//! # Cross-agent conformance contract
//!
//! The Claude transcript path is one [`TraceBackend`] behind a versioned,
//! testable contract. Other agents' logs are scored by adding
//! a backend that maps their transcript to a [`Trace`](aoa_trace::Trace) and
//! declares its native-vs-reconstructed provenance; [`run_conformance`]
//! validates any backend against [`CONTRACT_VERSION`]. [`GenericLogBackend`] is
//! a reconstructed example proving the contract generalizes beyond Claude.
//!
//! # Secrets
//!
//! This shim does **not** sanitize. Tool targets are lifted verbatim into span
//! attributes — the full `Bash` `command` and the `Read`/`Edit`/`Write` paths.
//! codeprobe strips secrets upstream, both when it writes `agent_output.txt`
//! and through the content policy it applies to every `trace.db` field, so
//! callers MUST point this at codeprobe's own outputs; feeding raw agent output
//! would carry any inline secret straight into the emitted trace. See
//! [`parse_transcript`] § Secrets.

mod backend;
mod error;
mod mapping;
mod parse;
mod reconstructed;
mod spans;
mod trace_db;

pub use backend::{
    run_conformance, ClaudeStreamJson, ConformanceOutcome, TraceBackend, CONTRACT_VERSION,
};
pub use error::{ShimError, TraceDbError};
pub use mapping::bash_runs_tests;
pub use parse::{parse_transcript, parse_transcript_file, read_capped, read_capped_framed};
pub use reconstructed::GenericLogBackend;
pub use spans::ShimResult;
pub use trace_db::{
    parse_trace_db, parse_trial, ParsedTrial, TraceDbTrial, TraceSource, MAX_TRACE_DB_BYTES,
    TRACE_DB_SCHEMA,
};
