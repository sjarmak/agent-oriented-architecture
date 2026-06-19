# Plan — unit-trace-substrate

## Files
1. `Cargo.toml` (workspace root)
   - `[workspace] members = ["crates/*"]`, `resolver = "2"`.
   - `[workspace.package]` shared metadata (version, edition=2021, rust-version=1.94).
   - `[workspace.dependencies]` for serde, serde_json, thiserror (pinned) so siblings inherit.
2. `.gitignore` — append `target/` and `.aoa/` (keep existing beads/dolt entries).
3. `crates/aoa-trace/Cargo.toml` — inherits workspace deps.
4. `crates/aoa-trace/schema/trace.schema.json` — published JSON Schema.
5. `crates/aoa-trace/src/` (small, focused files):
   - `span_type.rs` — `SpanType` enum (8 serde-renamed variants) + `SpanSource` enum.
   - `model.rs` — `Span`, `Trace` serde structs.
   - `error.rs` — `TraceError` (thiserror).
   - `report.rs` — `TraceReport` (per-type counts map + `has_reconstructed`).
   - `validate.rs` — `validate_trace(path)`: load, parse, check monotonic `seq`, build report.
   - `lib.rs` — module wiring + re-exports + embedded schema via `include_str!`.
6. `crates/aoa-trace/tests/`
   - `fixtures/valid.json`, `fixtures/out_of_order.json`, `fixtures/reconstructed.json`,
     `fixtures/invalid_schema.json`.
   - `validation.rs` — integration tests for AC5/AC6.
   - unit tests for AC3 (discriminant strings) live in `span_type.rs` + AC6 round-trip.

## Acceptance criteria mapping
- AC1: workspace + `cargo build --workspace`.
- AC2: `.gitignore` has `target/` and `.aoa/`.
- AC3: unit test asserting 8 exact discriminant strings.
- AC4: schema file committed + `include_str!` exposed as `pub const TRACE_SCHEMA`.
- AC5: `validate_trace` Ok(report w/ counts) for valid; Err for out-of-order + schema-invalid.
- AC6: `source` field round-trip + `has_reconstructed` surfaced.
- AC7: `cargo test -p aoa-trace` green.

## Validation strategy
Hand-rolled structural validation (serde parse + explicit monotonic/enum checks). The published
JSON Schema is for external consumers; runtime keeps deps minimal.
