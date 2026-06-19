# Research — unit-trace-substrate

## Repo state
- Greenfield Rust project ("AOA Toolkit"). No existing source, no Cargo workspace yet.
- Toolchain present: rustc/cargo 1.94.1 (target edition 2021).
- Issue tracking via `bd` (beads); `.gitignore` already ignores `.dolt/`, `*.db`, `.beads-credential-key`.

## Conventions (from CLAUDE.md / AGENTS.md + global rules)
- Many small files, high cohesion. 200-400 lines typical.
- Immutable patterns; comprehensive error handling via `thiserror`; no placeholder/stub code; no narration comments.
- Tests ship in the same commit as the source.
- Pin dependency versions.

## Task shape (R1 — Trace-event substrate)
- Cargo workspace root + first crate `aoa-trace` (the substrate every other crate depends on).
- 8 span discriminants (exact strings): `retrieval.search`, `file.read`, `symbol.lookup`,
  `write.attempt`, `write.blocked`, `test.run`, `gateway.invoke`, `abstain`.
- Published JSON Schema (`schema/trace.schema.json`), embedded via `include_str!`.
- serde models + ordered-span validation + `validate_trace(path) -> Result<TraceReport, TraceError>`.
- `source` field per span: `native` | `reconstructed`; report surfaces `has_reconstructed`.

## Design decisions
- Trace file format: JSON object `{ "spans": [ ... ] }` (documented in schema). Simple, extensible.
- Ordering key: `seq: u64`, validated monotonically non-decreasing.
- Validation: hand-rolled structural validation in Rust (serde does the structural parse;
  explicit checks add the ordering + enum constraints). Avoids pulling a heavy `jsonschema`
  dependency for the runtime path while still shipping the published schema for external consumers.
  This keeps the dependency tree minimal (serde, serde_json, thiserror only) per YAGNI.
- Public API: `SpanType`, `SpanSource`, `Span`, `Trace`, `TraceReport`, `TraceError`, `validate_trace`.
