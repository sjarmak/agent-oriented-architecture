# 0002 — `aoa-trace` owns the trace wire format

**Status:** Accepted. Recorded here in 2026-08 from the decision as it already
stands in `crates/aoa-trace` and `CLAUDE.md`'s conventions.

## Context

A trace file is the seam between two projects. `codeprobe` and the observe shim
write it; eight crates in this workspace read it. Both sides need one answer to
"what is a valid trace", and both sides are free to be wrong about it
independently — a producer can emit a field no reader knows, and a reader can
assume a field no producer writes. Neither mistake shows up as a compile error;
both show up as a metric that silently reads nothing.

## Decision

`aoa-trace` is the single owner of the trace wire format. The JSON Schema lives
at `crates/aoa-trace/schema/trace.schema.json` and is published to external
producers and consumers as `aoa_trace::TRACE_SCHEMA`, embedded at compile time.
Rust types in the same crate deserialize against it with
`#[serde(deny_unknown_fields)]`, so a field the schema does not describe is a
loud parse failure at the boundary rather than a value that quietly disappears.

No other crate restates the format. Crates that consume traces depend on
`aoa-trace` and validate through `aoa_trace::validate_trace` /
`validate_trace_value`.

This is the trace-file instance of the general convention in `CLAUDE.md`: state
wire formats once in their owning crate, and use `deny_unknown_fields` at
operator-authored input boundaries where schema drift must fail.

## Consequences

Adding a span attribute is a change to `aoa-trace` first; a consumer cannot
reach for a field the schema does not publish. That ordering is what makes the
schema readable as the definition of the format rather than as a lagging
description of it.

Format ownership is not the same as contract ownership, and the two version
independently. `aoa-codeprobe-shim::CONTRACT_VERSION` governs the *backend*
contract — the trait shape and the conformance invariants a backend must
satisfy — and is deliberately distinct from the wire schema. A backend stamps
the contract revision it was built against and the harness rejects a drifted
stamp; that mechanism says nothing about whether the bytes on disk are a valid
trace, which is the schema's job.

The published schema keeps `version` optional so that legacy unversioned trace
files stay schema-valid. That is a compatibility commitment, not an oversight:
tightening it invalidates preserved campaign artifacts.

An open failure this decision does not by itself prevent: `aoa-6w93u` reports
that the published schema validates traces no metric can actually read, meaning
producer-side validity and consumer-side usefulness have drifted apart while
both sides remain internally consistent. Single ownership makes that gap
locatable; it does not close it.

## Where this lives

- `crates/aoa-trace/src/lib.rs` — `TRACE_SCHEMA`, and the tests asserting what
  the schema must publish.
- `crates/aoa-trace/schema/trace.schema.json` — the schema itself.
- `crates/aoa-trace/src/envelope.rs`, `src/model.rs` — the
  `deny_unknown_fields` boundary.
- `crates/aoa-codeprobe-shim/src/backend.rs` — the backend contract version and
  why it is separate.
