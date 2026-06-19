# Research — unit-metrics

## aoa-trace span model (crates/aoa-trace/src)

Public API (re-exported from lib.rs): `SpanType`, `SpanSource`, `Span`, `Trace`, `validate_trace`, `validate_trace_value`, `TraceReport`, `TraceError`, `TRACE_SCHEMA`.

### SpanType (span_type.rs) — eight stable wire discriminants
- `retrieval.search` (RetrievalSearch)
- `file.read` (FileRead)
- `symbol.lookup` (SymbolLookup)
- `write.attempt` (WriteAttempt)
- `write.blocked` (WriteBlocked)
- `test.run` (TestRun)
- `gateway.invoke` (GatewayInvoke)
- `abstain` (Abstain)

`SpanSource`: `native` | `reconstructed`.

### Span (model.rs)
```
Span { span_type: SpanType (serde "type"), source: SpanSource, seq: u64, attributes: Map<String,Value> }
Trace { spans: Vec<Span> }
```
- Ordering key: `seq` (monotonic). We order by `seq` for "before first write" logic.
- `attributes` is free-form per span type. For metrics we read attribute keys:
  - retrieval/file/symbol spans carry a `symbol` and/or `path` attribute identifying the artifact touched.
  - retrieval.search may carry a ranked `results` array (ordered list of symbols/paths) — used for Recall@k / MRR over the first ranked batch.

## Four-metric definitions (report.md)

1. **Retrieval locality** (line 71, anchoring note line 270): gold set `G_t` of base-repo symbols. Measure tool-calls-before-first-`G_t`-access, Recall@k and MRR over the first ranked retrieval batch. CRITICAL (line 270): `G_t` defined on base-repo symbols is partly circular because retrieval-optimized variants move paths — anchor `G_t` to base-repo *symbols* and map through the transform map (`base_to_migrated`) to the migrated names the trace actually references.

2. **Edit locality** (line 73, line 271): inflation `|F_edit| / |F_min|`. `F_min` is not unique → report inflation against BOTH the **intersection** (floor) and **union** (ceiling) of >=2 accepted solutions. Condition on success.

3. **Invariant discoverability** (line 75, line 272): invariant set `I_t`. Measure whether any `I_t` artifact was accessed (file.read / symbol.lookup span) BEFORE the first `write.attempt`. Hold artifacts constant, vary placement.

4. **Mutation surface** (task spec): count of writable files reachable in the SCIP-style graph at depth <= k. Emit integer `k` and `over_approximation: true`. We model the graph ourselves — no real SCIP indexer.

## R15 — build-quality tiering
SCIP-grade index → `high-confidence`; best-effort index → `low-confidence`.

## R-silent — coverage tiering as a blocking gate
A degraded/empty index:
- lowers a score's weight (weight < 1.0),
- NEVER improves mutation-surface (degraded → smaller/empty reachable set, never larger),
- disqualifies repo from R0 voting → `repo_eligible_for_r0 = false`.

## Conditioning on HELD-OUT success
Every record carries `conditioned_on: held_out`. Input carries a `held_out_success: bool`. A task that visibly passes but fails the held-out check is NOT counted as success (`counted_as_success == false`).

## Decisions
- Model `SymbolGraph { nodes: Vec<String>, edges: Vec<(String,String)>, writable: Set<String>, quality: IndexQuality }`.
- `IndexQuality::Scip` → High confidence; `IndexQuality::BestEffort` → Low; `IndexQuality::Degraded`/empty → Low + ineligible for R0.
- Mutation-surface BFS over directed edges from graph roots (all nodes) to writable files at depth <= k. Degraded index = empty reachable set (never improves).
- All thresholds/k emitted as data (ZFC: mechanism, deterministic arithmetic, no judgment).
- Deps: aoa-trace (path), serde, serde_json, thiserror.
