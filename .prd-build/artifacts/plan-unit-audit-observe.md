# Plan — unit-audit-observe

## Crate layout (`crates/aoa-audit`)
- `Cargo.toml` — name aoa-audit; path deps aoa-trace/aoa-budget/aoa-metrics; serde, serde_json, thiserror; dev: tempfile (pinned).
- `src/lib.rs` — module wiring + re-exports + crate docs.
- `src/error.rs` — `AuditError` (thiserror): Io, Trace(#[from]), Budget(#[from]), Metric(#[from]).
- `src/tier.rs` — `Tier::{Tier1,Tier2,Tier3}` (serde, Ord by severity), `EnforcementPlane::{RuntimeHook,PreCommit,Ci}`.
- `src/observe.rs` — `observe(repo) -> Result<ObserveOutcome, AuditError>`. Creates `.aoa/traces/`, writes a local `.aoa/.gitignore` (`*`) so nothing under `.aoa/` is ever tracked. `ObserveOutcome { traces_dir, gitignore }`. Plus `write_trace(outcome, name, &Trace)` helper returning the path it wrote, used by acceptance #2.
- `src/planes.rs` — structural file-existence checks for the 3 enforcement planes (runtime hook config, pre-commit hook, CI workflow). Returns `Vec<MissingPlane>` (plane + which paths were probed).
- `src/punch.rs` — `PunchItem { title, tier, measured_cost: MeasuredCost, plane: Option<EnforcementPlane> }`; `MeasuredCost { value: u64, unit }` (real measured number). Ranking: by tier asc (Tier1 first) then measured cost desc, deterministic title tiebreak.
- `src/report.rs` — `AuditReport { items: Vec<PunchItem> }`, `render_human() -> String`, serde Serialize/Deserialize. `exit_code(&AuditReport, fail_on_tier1) -> i32`.
- `src/audit.rs` — `AuditConfig { context_root: Option<PathBuf>, ceiling, target, graph: SymbolGraph, k }` + `Default`. `audit(repo, &AuditConfig)`:
  1. context-file token closure via budget (oversized → Tier-2 item, cost = overflow tokens).
  2. retrieval-locality proxy + mutation-surface proxy via metrics (mutation surface → Tier-2 item, cost = writable_reachable). Read-only.
  3. missing enforcement planes (structural) → punch items with mapped tiers + plane.
  Sorts items, returns AuditReport. READ-ONLY: never writes.

## Tier mapping (from research)
- RuntimeHook missing → Tier1. Ci missing → Tier1. PreCommit missing → Tier2.
- Oversized context closure → Tier2. Mutation surface → Tier2.

## exit_code
`0` unless `fail_on_tier1 && any item.tier == Tier1`. Then non-zero (2).

## Tests (tests/acceptance.rs) — 7 criteria
1. observe writes nothing tracked: temp repo, snapshot file set before, run observe, only `.aoa/**` new.
2. observe path produces a valid trace: observe → write_trace(valid Trace) → validate_trace passes.
3. audit writes nothing: snapshot file set before/after over temp fixture, unchanged.
4. both renderings: render_human() non-empty + has cost; serde_json round-trips.
5. every item has a tier (enum, always present — assert non-empty items each Some tier via match).
6. table-driven exit code over 4 combos.
7. whole suite green via `cargo test -p aoa-audit`.
