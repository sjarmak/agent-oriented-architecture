# Test Results — unit-trace-substrate

## Commands
- `cargo build --workspace` → Finished, 0 errors.
- `cargo test -p aoa-trace` → 9 passed, 0 failed (3 unit + 6 integration + 0 doc).
- `cargo clippy --workspace --all-targets` → clean, 0 warnings.

## Test inventory
Unit (`src/span_type.rs`, `src/lib.rs`):
- `span_types_serialize_to_exact_discriminants` — all 8 variants serialize to exact
  discriminant strings + deserialize round-trip; `ALL.len() == 8`. (AC3)
- `span_source_round_trips` — `native`/`reconstructed` round-trip. (AC6)
- `embedded_schema_is_valid_json_describing_spans` — `include_str!` schema parses and
  describes `spans`. (AC4)

Integration (`tests/validation.rs`):
- `valid_trace_reports_per_type_counts` — Ok with per-type counts + total. (AC5)
- `out_of_order_trace_is_rejected` — Err::OutOfOrder at expected index/seq. (AC5)
- `schema_invalid_trace_is_rejected` — Err::Schema for unknown discriminant. (AC5)
- `reconstructed_span_is_surfaced_and_round_trips` — `has_reconstructed()` true +
  span source round-trip. (AC6)
- `missing_file_returns_read_error` — Err::Read for absent file.
- `equal_seq_is_allowed` — monotonic non-decreasing accepts equal seq.

## Acceptance criteria
| AC | Status | Evidence |
|----|--------|----------|
| 1  | PASS | `members=["crates/*"]`; `cargo build --workspace` ok |
| 2  | PASS | `.gitignore` contains `target/` and `.aoa/` |
| 3  | PASS | `span_types_serialize_to_exact_discriminants` |
| 4  | PASS | `schema/trace.schema.json` + `TRACE_SCHEMA` via `include_str!` |
| 5  | PASS | valid/out_of_order/invalid_schema fixtures + tests |
| 6  | PASS | `source` field, `has_reconstructed()`, round-trip test |
| 7  | PASS | `cargo test -p aoa-trace` → 0 failures |
