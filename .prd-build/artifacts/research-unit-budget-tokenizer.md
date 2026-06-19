# Research — unit-budget-tokenizer

## Workspace conventions (from aoa-trace)
- Crates live under `crates/*`; root `Cargo.toml` uses `members = ["crates/*"]` and `[workspace.dependencies]` (serde, serde_json, thiserror).
- Crate Cargo.toml inherits `version.workspace = true`, `edition.workspace = true`, `rust-version.workspace = true`, `license.workspace = true`, and deps via `{ workspace = true }`.
- aoa-trace style: many small modules (`error.rs`, `model.rs`, `report.rs`, `validate.rs`), `lib.rs` re-exports public types with module-level `//!` doc. Errors use `thiserror`. Tests in `tests/` with committed fixtures under `tests/fixtures/`.
- Toolchain: rustc 1.94.1, edition 2021.

## tiktoken-rs API (v0.12.0)
- `tiktoken_rs::o200k_base() -> Result<CoreBPE>` — the pinned reference encoding.
- `tiktoken_rs::cl100k_base() -> CoreBPE`-style constructors also exist (`p50k_base`, `r50k_base`).
- Counting: `CoreBPE::encode_with_special_tokens(text: &str) -> Vec<usize>`; token count = `.len()`.
- We map a target-model NAME (string) to an encoding. Unknown name => `Err` (criterion 7, fail loud). Supported target names: `o200k_base`/GPT-4o/GPT-4.1 family, `cl100k_base`/GPT-4/3.5 family. The reference is always o200k_base.

## Reference syntax for closure resolution
- Markdown links: `[label](relative/path)` — capture the path inside `()`.
- `@path` includes (CLAUDE.md/AGENTS.md style): a line/token beginning with `@` followed by a relative path.
- Paths resolved relative to the referencing file's directory; only resolve local (non-URL, non-anchor) markdown links. Skip `http(s)://`, `mailto:`, and `#anchor` targets.
- Cycle-safe BFS/DFS tracking visited canonical paths. Multi-hop A->B->C must be reached.

## Decisions
- Public API: `resolve_closure(root) -> Result<Closure, BudgetError>`, `count_budget(&Closure, target) -> Result<BudgetReport, BudgetError>`, plus a `Config` (ceiling, warn-first, changed-files scope) and `fix` op.
- Suppression: inline `# aoa-allow: oversized-context <reason>` per-file marker captured into report; suppresses that file's contribution to a Block verdict.
- `fix`: deterministic mechanical split of an over-budget file into linked sub-files (no LLM), then re-resolve + re-count to green.
