# Test Results — unit-budget-tokenizer

`cargo test -p aoa-budget` — 9 passed, 0 failed.
`cargo build --workspace` — clean.
`cargo clippy -p aoa-budget --all-targets` — no warnings.
`cargo fmt -p aoa-budget` — applied.

## Acceptance criteria coverage
1. Multi-hop closure A->B->C — `resolves_multi_hop_closure` (AGENTS.md -> rules/README.md via markdown link -> rules/deep.md via `@deep.md` include). `skips_external_and_anchor_references` confirms http(s)/anchor targets are not followed; deep.md links back to root proving cycle-safety.
2. Real tokenizer, dual counts — `reports_dual_tokenizer_counts` asserts both `o200k_tokens` and `target_tokens` (>0) totals and per-file. `target_tokenizer_uses_distinct_encoding_for_cl100k` proves cl100k is a genuinely different encoding (CJK probe diverges from o200k) and that the o200k target mirrors the pinned reference. tiktoken-rs 0.12.0 pinned.
3. Verdict table — `verdict_table`: under->Pass (both modes), over+default->Block, over+warn_first->Warn. Gated on target tokens.
4. Suppression — `suppression_marker_suppresses_and_captures_reason`: ceiling 1 would Block, but `# aoa-allow: oversized-context ...` yields Pass, gating sum 0, and the captured reason (AOA-123) appears via `report.suppressions()`.
5. Diff scope — `diff_scope_gates_only_changed_files`: only the file in the changed-file set is `gating`; the unchanged file is reported but excluded from the gate sum.
6. Fix to green — `fix_oversized_file_rechecks_green`: fixture starts Block at ceiling 200; `fix_oversized` extractively summarizes the body (deterministic, no LLM), archives the full original to `<stem>.archive.md` (kept out of the active closure), re-resolves and re-counts to Pass with `gating_target_tokens < 200`.
7. Unknown tokenizer fails loud — `unknown_target_tokenizer_errors`: `count_budget(.., "totally-made-up-model", ..)` returns `Err(BudgetError::UnknownTargetTokenizer)`, never a silent default.
8. `cargo test -p aoa-budget` green (9/9).

## Design notes
- Verdict is gated on the target tokenizer's sum over in-scope, non-suppressed files; both o200k and target totals are always reported.
- `fix` summarizes rather than splits: pure splitting into linked files cannot reduce a closure's summed tokens (the parts re-sum), so it would never re-check green against a sum-based gate. Extractive summarization with an unreferenced archive reduces closure tokens deterministically and re-checks to Pass.
