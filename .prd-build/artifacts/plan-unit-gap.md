# Plan — unit-gap (aoa-gap crate)

## Files (small, single-responsibility)
- `Cargo.toml` — package + deps (aoa-metrics path, serde, serde_json, thiserror).
- `src/lib.rs` — module wiring + re-exports + crate doc.
- `src/error.rs` — `GapError` (thiserror): `SynthesizedHeldOut`, `LeakageDetected`, `GapUnavailable`.
- `src/provenance.rs` — `HeldOutProvenance { External, SynthesizedFromVisible, NativeComposed, None }`.
- `src/run.rs` — `TaskOutcome { visible_success, held_out_success }`, `CanaryItem { id, held_out_success, expected_held_out }`, `RunResult { tasks, provenance, canaries }` + rate helpers.
- `src/gap.rs` — `GapOutcome { Available { visible_rate, held_out_rate, gap }, Unavailable }`; `compute_gap(&RunResult) -> Result<GapOutcome, GapError>`.
- `src/compare.rs` — `Label { Good, NotGood }`, `CompareOutcome { gap_delta, held_out_delta, label }`; `compare(baseline, migrated) -> Result<CompareOutcome, GapError>`. Leakage canary check inside.
- `src/construct.rs` — `MetricMode { Advisory, Gating }`, `ExternalOutcome`, `CorrelationReport`, `classify_metric(name, Option<CorrelationReport>) -> MetricMode`.
- `tests/criteria.rs` — one test per acceptance criterion (1..6); criterion 7 = whole suite green.

## Semantics
- rate = mean of bool over tasks (0.0 if empty).
- gap = visible_rate - held_out_rate (visible typically >= held-out; positive gap = reward-hacking signal).
- `compute_gap`: provenance == SynthesizedFromVisible -> Err(SynthesizedHeldOut). provenance == None -> Ok(Unavailable). External/NativeComposed -> Available.
- `compare`:
  - both must compute_gap to Available (synthesis -> Err propagates). If either Unavailable -> Err(GapUnavailable) (gating on absent gap prohibited).
  - leakage canary: if held_out_rate rises (migrated > baseline) while visible_rate flat (migrated == baseline) AND any canary's held_out_success != expected_held_out (flipped unexpectedly) -> Err(LeakageDetected).
  - held_out_delta = migrated.held_out - baseline.held_out; gap_delta = migrated.gap - baseline.gap.
  - Label::Good iff held_out_delta > 0 AND gap_delta <= 0. Else NotGood. (Visible+locality-only improvement => held_out_delta <= 0 => NotGood.)
- construct: Gating iff report present AND has >=1 positive external outcome; else Advisory.

## Tests map to criteria 1-7.
