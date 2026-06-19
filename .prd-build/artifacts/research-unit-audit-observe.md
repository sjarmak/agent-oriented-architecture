# Research — unit-audit-observe

## Dep crate public APIs (exact)

### aoa-trace
- `validate_trace(path: &Path) -> Result<TraceReport, TraceError>` — reads + parses + ordering check.
- `validate_trace_value(&Trace) -> Result<TraceReport, TraceError>`.
- `Trace { spans: Vec<Span> }`, `Span { span_type: SpanType, source: SpanSource, seq: u64, attributes: Map<String,Value> }`.
- `SpanType` 8 variants, wire strings via `#[serde(rename=...)]`: `retrieval.search`, `file.read`, `symbol.lookup`, `write.attempt`, `write.blocked`, `test.run`, `gateway.invoke`, `abstain`.
- `SpanSource::{Native, Reconstructed}` (lowercase wire).
- `Span`/`Trace` derive Serialize+Deserialize → we can construct a Trace in code and serialize to JSON, then `validate_trace(path)`.

### aoa-budget
- `resolve_closure(root: &Path) -> Result<Closure, BudgetError>`; `Closure { root, files: Vec<ContextFile{path,text}> }`.
- `count_budget(&Closure, target: &str, &Config) -> Result<BudgetReport, BudgetError>`.
- `Config::blocking(ceiling)` / `Config::warn_first(ceiling)`; fields `ceiling`, `warn_first`, `changed_files`.
- `BudgetReport { o200k_tokens, target_tokens, gating_target_tokens, ..., verdict, files: Vec<FileBudget> }`; `FileBudget { path, o200k_tokens, target_tokens, gating, suppression }`.
- Target tokenizer names: `o200k_base`/`cl100k_base` + model aliases (gpt-4o etc). Unknown → `BudgetError::UnknownTargetTokenizer`. We use `"o200k_base"` (always available, no network).
- Verdict::{Pass,Warn,Block}. Over-budget context file = a real measured token cost.

### aoa-metrics
- `SymbolGraph { nodes: Vec<String>, edges: Vec<(String,String)>, writable: BTreeSet<String>, quality: IndexQuality }` — modeled in-crate, NO real SCIP indexer.
- `IndexQuality::{Scip, BestEffort, Degraded}`.
- `compute_mutation_surface(&MetricInput) -> MutationSurface { writable_reachable, reachable, k, ... }` — count of writable files reachable at depth <=k = mutation-surface proxy.
- `compute_retrieval_locality(&MetricInput) -> RetrievalLocality { tool_calls_to_first_relevant_artifact, recall_at_k, mrr, ... }` — retrieval-locality proxy.
- `MetricInput { trace, gold_set, invariant_set, transform, edited_files, accepted_solutions, graph, k, held_out_success }`.

## Tier framework (report.md §"tightened, evidence-grounded best-practices framework")
- **Tier 1 (adopt now, evidence-backed):** trace telemetry (harness lever), retrieval/localization first-class, abstention/reproduction-before-mutation, capped+linted context files.
- **Tier 2 (hypotheses, pilot & measure):** context-optimum calibration, typed contracts at boundaries, narrow mutation gateways / protected paths / ownership.
- **Tier 3 (asserted, unsupported):** specific quantitative claims (context-cost %, ADR detection, CODEOWNERS speed).

### Enforcement-plane → tier mapping (documented decision)
- **Runtime hook** (write.attempt/write.blocked telemetry + mutation gateway) → Tier-1: it is the trace-telemetry + reproduction-before-mutation lever that the evidence puts first.
- **CI workflow** (runnable checks + context-smell lint as CI findings) → Tier-1: Tier-1 item 4 says treat context smells as CI findings; runnable checks are the harness lever.
- **Pre-commit hook** (local capped-context lint before push) → Tier-2: a convenience plane; the gate that *must* hold is CI. Pre-commit is the pilot-and-measure local mirror.
- **Oversized context file** punch item → Tier-2 (calibrate to optimum, item 5), measured cost = target token overflow above ceiling.
- **Mutation surface** punch item → Tier-2 (narrow mutation gateways, item 7), measured cost = count of writable reachable files.

## Workspace facts
- Root `Cargo.toml`: `members = ["crates/*"]` → new crate auto-joins, do NOT edit root.
- `.gitignore` already ignores `.aoa/` → observe writing under `.aoa/traces/` is already ignored repo-wide; we still ensure a local `.gitignore` entry inside `.aoa/` for hermetic temp repos with no root ignore.
- Workspace deps available: serde, serde_json, thiserror. tiktoken-rs pulled transitively via aoa-budget.
- tempfile must be pinned as a dev-dependency.
