# Test results — unit-metrics

## Commands
- `cargo test -p aoa-metrics` → **13 passed; 0 failed**
- `cargo build --workspace` → Finished, no errors
- `cargo clippy -p aoa-metrics --all-targets -- -D warnings` → clean
- `cargo fmt -p aoa-metrics -- --check` → clean

## Acceptance criteria → tests

| # | Criterion | Test(s) |
|---|-----------|---------|
| 1 | retrieval-locality emits tool-calls-to-first-relevant, Recall@k, MRR; G_t anchored via transform-map (RENAME) | `retrieval_locality_anchors_gold_through_rename` (gold `OrderService` → `orders::Service`, raw name does NOT match; anchored does), `retrieval_locality_misses_when_only_raw_name_present` |
| 2 | edit-locality emits inflation vs BOTH intersection floor AND union ceiling of >=2 solutions; floor <= ceiling | `edit_locality_emits_floor_and_ceiling` (intersection=1, union=3, floor_inflation=1.0 <= ceiling_inflation=3.0), `edit_locality_requires_two_solutions` |
| 3 | invariant-discoverability true when I_t access precedes first write.attempt, false otherwise | `invariant_discovered_before_write`, `invariant_not_discovered_when_accessed_after_write` |
| 4 | mutation-surface counts writable files reachable at depth <= k; emits integer k and over_approximation: true | `mutation_surface_counts_reachable_and_emits_k`, `mutation_surface_respects_depth_bound` |
| 5 | every record carries conditioned_on: held_out; visible-pass-but-held-out-FAIL not counted as success | `records_conditioned_on_held_out_success` |
| 6 | high-confidence (SCIP) vs low-confidence (best-effort); degraded lowers weight, never raises mutation-surface, sets repo_eligible_for_r0=false | `scip_index_is_high_confidence_full_weight_and_eligible`, `best_effort_index_is_low_confidence_lower_weight`, `degraded_index_lowers_weight_disqualifies_r0_and_never_raises_surface` |
| 7 | `cargo test -p aoa-metrics` passes with 0 failures | whole suite (13/13) |

Plus `transform_map_loads_from_fixture` covering fixture deserialization.

## Notes
- `floor_inflation = |F_edit|/|union|` (looser), `ceiling_inflation = |F_edit|/|intersection|` (stricter), so floor_inflation <= ceiling_inflation by construction; intersection_size <= union_size also emitted.
- Degraded index → empty reachable set in mutation-surface (BFS short-circuits), weight 0.0, repo_eligible_for_r0=false. Confirmed degraded surface <= SCIP surface.
- Artifact identity read from span `symbol` then `path` attribute; ranked retrieval batch from `results` array.
