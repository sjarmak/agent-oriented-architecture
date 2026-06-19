# Test results — unit-audit-observe

## Commands
- `cargo test -p aoa-audit` → **7 passed, 0 failed**.
- `cargo build --workspace` → clean.
- `cargo fmt -p aoa-audit --check` → clean.
- `cargo clippy -p aoa-audit --all-targets` → no warnings, no errors.

## Acceptance criteria → test mapping
1. observe writes nothing tracked → `observe_writes_only_ignored_aoa_tree` (temp repo, before/after file-set diff, only `.aoa/**` added). PASS
2. observe path yields a valid trace → `observe_path_produces_valid_trace` (write_trace then standalone `aoa_trace::validate_trace`). PASS
3. audit writes nothing tracked → `audit_does_not_mutate_repo` (file-set equality before/after over temp fixture). PASS
4. both renderings → `audit_emits_human_and_json_renderings` (human has `cost:`; JSON round-trips and deserializes). PASS
5. every item tiered → `every_item_has_a_tier` (matches Tier1|Tier2|Tier3). PASS
6. exit-code semantics → `exit_code_table` (4-combo table: only fail_on_tier1 && tier1-present → 2, else 0). PASS
7. suite green → `cargo test -p aoa-audit` 0 failures. PASS
- Extra: `default_audit_on_bare_repo_is_well_formed` checks ranking invariant (tiers non-decreasing).

## Notes
- Used `o200k_base` as the budget target tokenizer (loads offline, pinned reference encoding).
- Symbol graph modeled in-crate via `aoa_metrics::SymbolGraph`; no real SCIP indexer is invoked.
- Did not edit root Cargo.toml or any other crate. New crate auto-joins via `members = ["crates/*"]`.
