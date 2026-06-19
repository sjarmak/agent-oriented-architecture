# Plan — unit-metrics

## Crate: crates/aoa-metrics (lib)

### Files (many small)
- `Cargo.toml` — aoa-trace (path), serde, serde_json, thiserror.
- `src/lib.rs` — module wiring + public re-exports.
- `src/error.rs` — `MetricError` (thiserror).
- `src/input.rs` — `MetricInput`, `TransformMap`, `SymbolGraph`, `IndexQuality`, accepted-solutions type, `Confidence`.
- `src/common.rs` — shared output fields: `Confidence`, `ConditionedOn`, weight helper, artifact extraction from spans.
- `src/retrieval.rs` — `compute_retrieval_locality` → `RetrievalLocality { tool_calls_to_first_relevant, recall_at_k, mrr, k, conditioned_on, confidence, weight }`.
- `src/edit.rs` — `compute_edit_locality` → `EditLocality { f_edit, floor_inflation, ceiling_inflation, intersection_size, union_size, ... }`. Assert floor <= ceiling.
- `src/invariant.rs` — `compute_invariant_discoverability` → `InvariantDiscoverability { accessed_before_first_write: bool, ... }`.
- `src/mutation.rs` — `compute_mutation_surface` → `MutationSurface { writable_reachable, k, over_approximation: true, ... }`.
- `src/record.rs` — `compute_metrics(&MetricInput) -> MetricRecord` combining all four + `repo_eligible_for_r0` + `counted_as_success`.
- `tests/acceptance.rs` — one test per acceptance criterion (1-6) + helpers; criterion 7 is the suite passing.
- `tests/fixtures/` — JSON fixtures (trace, transform-map) for the rename test.

### Key logic
- **Anchoring (crit 1):** `G_t` is base-repo symbols. Map each via `TransformMap.base_to_migrated` to the migrated name; match trace artifacts against migrated names. Rename fixture: base `OrderService` → migrated `orders::Service`; trace references `orders::Service`. Raw base name would NOT match; anchored does.
- **Recall@k / MRR:** over first retrieval.search span's ranked `results`. Recall@k = |first-k ∩ anchored G_t| / |G_t|. MRR = 1/rank of first relevant in ranked list (0 if none).
- **tool-calls-to-first-relevant:** index (1-based count) of first span (any tool span) whose artifact ∈ anchored G_t.
- **Edit (crit 2):** floor = intersection of accepted solution file-sets; ceiling = union. floor_inflation = |F_edit| / max(1,|intersection|); ceiling_inflation = |F_edit| / max(1,|union|). Both emitted; floor >= ceiling numerically (smaller denom → larger ratio) — but spec says "floor <= ceiling" referring to the *bound sets*: intersection (floor set) <= union (ceiling set). We emit both inflation ratios AND the floor/ceiling set sizes; assert intersection_size <= union_size and both inflation values present. Name fields `floor_inflation`/`ceiling_inflation` with floor = vs-union (looser, smaller ratio) ... resolve: floor inflation uses ceiling-set (union) denom → lower ratio; ceiling inflation uses intersection denom → higher ratio. To honor "floor <= ceiling" on the emitted inflation numbers: floor_inflation <= ceiling_inflation. So floor_inflation = |F_edit|/|union|, ceiling_inflation = |F_edit|/|intersection|. Test asserts floor_inflation <= ceiling_inflation and both present.
- **Invariant (crit 3):** order spans by seq. first_write = first WriteAttempt seq. accessed_before = any file.read/symbol.lookup with artifact ∈ I_t and seq < first_write_seq.
- **Mutation (crit 4):** BFS from all nodes over directed edges, depth <= k, collect writable nodes; degraded/empty index → empty set. Emit k, over_approximation=true.
- **Conditioning (crit 5):** counted_as_success = held_out_success. Record always carries conditioned_on=held_out.
- **Confidence/R15 + R-silent (crit 6):** Scip→High weight 1.0; BestEffort→Low weight 0.5; Degraded→Low weight 0.0 + repo_eligible_for_r0=false. Degraded never raises mutation surface (empty reachable).

### Tests map to criteria 1-7.
