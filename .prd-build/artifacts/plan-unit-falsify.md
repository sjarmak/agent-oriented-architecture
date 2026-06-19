# Plan — unit-falsify

## Module layout (crates/aoa-falsify/src)
- `error.rs` — `FalsifyError` (thiserror): `TooFewRepos`, `EmptyRuns`.
- `convention.rs` — `ScoringConvention` (edit-locality floor/ceiling toggle, mutation depth-k, metric weights). `admissible_default()` -> Vec of conventions. Each convention exposes `score_task(&PairTask) -> (bool /*repo_success*/, bool /*harness_success*/)` deterministically (held-out success counting under the convention).
- `types.rs` — `Confidence` re-export usage, `Eligibility { confidence, native_span, calibrated }`, `PairTask { task_id, is_identical_pair, repo_held_out_success, harness_held_out_success, edit_locality_floor, edit_locality_ceiling, mutation_depth }`, `RepoRun` snapshot, `RepoResult { repo_id, eligibility, runs: Vec<RepoRun>, holdout_size }`, `FalsifyConfig { k_runs, min_holdout_size, min_effect_size, conventions }`, `FalsifyInput`.
- `eligibility.rs` — `is_eligible(&Eligibility) -> bool` = High confidence AND native_span AND calibrated.
- `delta.rs` — per-repo held-out success deltas over identical-pair tasks under a given convention: `repo_delta`, `harness_delta`. Excludes non-paired tasks.
- `verdict.rs` — `Verdict`, single-run verdict from eligible repo votes (majority repo_delta>=harness_delta => Proceed, tie => Pivot), plus the R0' pipeline (determinism, convention-invariance, power) producing the final hardened verdict.
- `report.rs` — `FalsifyReport`, `falsify()`, `to_json()`.
- `lib.rs` — module wiring + re-exports + crate docs.

## Verdict pipeline (verdict.rs)
1. Filter eligible repos. If <5 eligible -> still compute but power/precondition handles significance.
2. Power precondition: if any voting basis below min_holdout_size or aggregate effect size below min_effect_size -> result is `Inconclusive` (cannot return significant verdict). Check BEFORE proceed can be granted.
3. For the default convention, compute per-run verdict for each of K runs (determinism). If K<1 -> error. If the K verdicts are not all equal -> downgrade any Proceed to Inconclusive.
4. Base verdict = run verdict (must be stable). 
5. If base == Proceed: convention-invariance — recompute verdict under each admissible convention (over the canonical run). If any != Proceed -> Inconclusive.
6. Eligibility already enforced by filtering. Need >=5 eligible repos AND strict majority for Proceed; else if exactly tie -> Pivot; else (minority) -> Pivot.
7. Inconclusive preserved verbatim — never mapped to Pivot.

## Report deltas
`repo_delta` / `harness_delta` in the report = mean across eligible repos of per-repo deltas under the default convention (deterministic arithmetic), for transparency. Verdict uses the per-repo vote comparison, not the means.

## Tests (tests/falsify_test.rs + fixtures)
1. emits falsification.json with repo_delta, harness_delta, verdict — fixture.
2. table-driven majority incl. tie => pivot.
3. only identical-pair tasks contribute.
4. determinism: unstable K runs => inconclusive not proceed.
5. convention-invariance: flip under a convention => inconclusive.
6. ineligible (low-confidence/reconstructed) repo excluded from voting.
7. power precondition: small holdout => inconclusive.
8. inconclusive preserved verbatim in json.
9. `cargo test -p aoa-falsify` green.

## Constraints
- No RNG. Determinism via provided seed/run snapshots only.
- Conventions emitted as data (in report.notes / conventions_tried).
- thiserror for structural errors only; downgrades are data.
- No modifying other crates or root Cargo.toml.
