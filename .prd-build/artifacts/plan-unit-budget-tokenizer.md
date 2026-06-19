# Plan — unit-budget-tokenizer

## Crate layout (`crates/aoa-budget`)
- `Cargo.toml` — inherit workspace; deps: tiktoken-rs = "0.12.0", serde, serde_json, thiserror.
- `src/lib.rs` — module decls + `//!` doc + public re-exports.
- `src/error.rs` — `BudgetError` (thiserror): Io, UnknownTargetTokenizer, FixFailed.
- `src/reference.rs` — extract references (markdown links + `@path`) from file text; classify local vs external.
- `src/closure.rs` — `Closure` type + `resolve_closure(root)` cycle-safe DFS over references.
- `src/tokenizer.rs` — `Tokenizer` abstraction; `reference_encoder()` (o200k_base), `target_encoder(name)` -> Result (unknown => Err).
- `src/suppress.rs` — parse `# aoa-allow: oversized-context <reason>` markers per file.
- `src/budget.rs` — `Config`, `Verdict`, `FileBudget`, `BudgetReport`, `count_budget(&Closure, target, &Config)`; verdict gated on target tokens; warn-first vs block; changed-files scoping; suppression.
- `src/fix.rs` — `fix_oversized(path, &Config, target)` deterministic split into linked sub-files; returns new file list; re-check green.

## Verdict logic (gated on target_tokens)
- sum target_tokens of *non-suppressed, in-scope* files.
- under ceiling => Pass.
- over ceiling: warn_first => Warn; else Block.
- suppressed files excluded from the gating sum; their reasons recorded.
- changed-files scope (Some(list)) => only those files count toward gate (others still listed but not gating).

## Tests (tests/budget.rs + fixtures)
1. multi-hop closure A->B->C.
2. dual tokenizer: both o200k + target counts present & > 0.
3. table-driven verdict: under=Pass, over+default=Block, over+warn_first=Warn.
4. suppression marker => not Block, reason captured.
5. changed-files scope flags only listed files.
6. fix splits over-budget file => re-check tokens < ceiling.
7. unknown target tokenizer => Err.
8. `cargo test -p aoa-budget` green.
