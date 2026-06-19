# Research — unit-gap

## Workspace
- Root `Cargo.toml`: `members = ["crates/*"]`, resolver 2, workspace deps for serde/serde_json/thiserror. Do NOT edit.
- Sibling crates use `version.workspace = true`, `edition.workspace`, etc. and pull deps via `{ workspace = true }`.

## aoa-metrics public surface (path dep target)
- `MetricRecord` (record.rs): carries the four locality metrics, `conditioned_on: ConditionedOn::HeldOut`, `counted_as_success` (held-out only), `confidence: Confidence`, `weight`, `repo_eligible_for_r0`.
- `Confidence { High, Low }`, `IndexQuality { Scip, BestEffort, Degraded }` (input.rs).
- `MetricError` via thiserror (error.rs).
- Crucial pattern echoed by the toolkit: success is conditioned on HELD-OUT, never visible. `counted_as_success = held_out_success`. This validates R9's premise: visible-pass alone is not success.

## Design implications for aoa-gap
- aoa-gap is its own concern: visible-vs-held-out *gap*, *leakage canary*, *construct validity*. It depends on aoa-metrics only as a path dep (per spec); it does not need to reuse metric internals. We reference `aoa_metrics::Confidence`/`MetricRecord` lightly so the dep is real and meaningful (classify_metric can carry a metric name + the record's confidence is not required). Keep coupling minimal — re-exporting nothing from aoa-metrics, just `use aoa_metrics;` where a real type is consumed.
- Provenance enum mirrors metrics' "held-out is authoritative" stance.

## ZFC
- All deterministic mechanism: rates are arithmetic means of per-task booleans; labels are boolean predicates over deltas. No semantic judgment, no thresholds beyond explicit non-increase / strict-improvement comparisons.
