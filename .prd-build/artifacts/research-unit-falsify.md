# Research — unit-falsify (R0 + R0')

## Goal
Create `crates/aoa-falsify` = the "wrong layer" falsification gate (R0) + robust/abstaining hardening (R0'). Depends on `aoa-gap` and `aoa-metrics` (path deps). Emits `falsification.json` with `repo_delta`, `harness_delta` (held-out success deltas) and a `verdict ∈ {proceed, pivot, inconclusive}`.

## Dependency surface (exact public types)

### aoa-metrics (`crates/aoa-metrics/src/`)
- `Confidence { High, Low }` (input.rs). `pub enum Confidence`.
- `IndexQuality { Scip, BestEffort, Degraded }` with:
  - `confidence() -> Confidence` (Scip => High).
  - `weight() -> f64`.
  - `eligible_for_r0() -> bool` (false only for Degraded).
- `MetricRecord { ..., repo_eligible_for_r0: bool }` (record.rs) — derived from `quality.eligible_for_r0()`.
- The crate doc explicitly says a degraded/low-confidence repo "disqualifies the repo from R0 voting (R-silent)" and that thresholds (Recall@k, mutation depth-k) are emitted as DATA. This is the ZFC anchor for emitting admissible conventions as data.

NOTE: there is NO free function literally named `repo_eligible_for_r0(...)`. It is a field on `MetricRecord` and a method `IndexQuality::eligible_for_r0()`. For aoa-falsify eligibility we need three independent facts: confidence (SCIP-grade => High), native-span, calibrated. Only confidence maps to an aoa-metrics type (`Confidence`). We reuse `aoa_metrics::Confidence` for the confidence dimension; native_span and calibrated are booleans carried on our own `Eligibility` struct. This keeps the dep meaningful (we vote only when `Confidence::High`) without inventing metrics-side types.

### aoa-gap (`crates/aoa-gap/src/`)
- `compare(baseline, migrated) -> Result<CompareOutcome, GapError>` where `CompareOutcome { gap_delta, held_out_delta, label }`, `Label { Good, NotGood }`.
- `GapOutcome::{Available { held_out_rate, gap, .. }, Unavailable}`.
- `HeldOutProvenance { External, SynthesizedFromVisible, NativeComposed, None }`.
- `RunResult` / `TaskOutcome` / `CanaryItem` from run.rs (held_out_rate(), visible_rate(), any_canary_flipped()).

We depend on aoa-gap conceptually (held-out success is the gap crate's currency; `Label`/`HeldOutProvenance` express the same held-out integrity stance). Concretely, aoa-falsify computes held-out success deltas directly over identical-pair tasks — the same "held-out is the only currency that counts" rule the gap crate enforces. We reference `aoa_gap::HeldOutProvenance` to require native-composed provenance as part of native-span eligibility, tying the dep in meaningfully.

## R0 mechanic (from task spec)
- Over >=5 repos compute, per repo:
  - `harness-delta`: swap harness, fixed repo. held-out success delta.
  - `repo-delta`: AOA migration, fixed harness. held-out success delta.
  - ONLY identical-pair tasks contribute. Non-paired tasks excluded.
- `proceed` iff repo-delta >= harness-delta on a MAJORITY of >=5 ELIGIBLE repos.
- exact tie (eligible majority count split evenly / no strict majority) => `pivot`.
- All arithmetic, deterministic.

## R0' hardening (verdict downgrades)
`proceed` only if ALL hold, else downgrade:
- (a) DETERMINISM: stable across K>=3 fixed-seed runs. Unstable => `inconclusive`.
- (b) CONVENTION-INVARIANCE: invariant across all admissible scoring conventions (edit-locality floor AND ceiling, mutation-surface depth-k, alternative metric weights). Flip under any => `inconclusive`. Conventions are DATA (emitted).
- (c) ELIGIBILITY: only high-confidence (SCIP) AND native-span AND calibrated repos vote.
- POWER precondition: holdout below min size / effect-size threshold => cannot return a significant verdict => `inconclusive`.
- `inconclusive` is NEVER silently converted to `pivot` — preserved verbatim.

## Public API (from design guidance)
- `pub fn falsify(input: &FalsifyInput) -> FalsifyReport`
- `FalsifyReport { repo_delta, harness_delta, verdict: Verdict, eligible_repos, excluded_repos, notes }`
- `Verdict ∈ {Proceed, Pivot, Inconclusive}` (serde snake_case: proceed/pivot/inconclusive).
- `FalsifyInput`: per-repo `RepoResult { repo_id, eligibility(confidence, native_span, calibrated), identical_pair_tasks: Vec<PairTask>, holdout_size }` + config (K, power thresholds, admissible conventions).

## Decisions
- Determinism modeled as caller supplying K run-instances (Vec of per-run task success sets) OR a deterministic seed-varied closure. We take K explicit run snapshots per repo; verdict computed per run; stability = all K verdicts equal. No RNG.
- Convention re-evaluation: each `ScoringConvention` re-weights/re-thresholds the held-out success counting; recompute verdict; if any flips away from proceed, downgrade. Conventions emitted in report notes.
- `thiserror` only for genuine error paths (e.g. <5 repos input = structural error). Verdict downgrades are NOT errors — they are data.
- serde-serializable structs; write `falsification.json` via `serde_json`.
- Many small files: types.rs, eligibility.rs, delta.rs, convention.rs, verdict.rs, report.rs, error.rs, lib.rs.
