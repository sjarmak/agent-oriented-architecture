# Gardening ledger

## 2026-10-18 — deletion

- SHA: `7416517e0507215d52cd143d941e22e1ef890ee7`
- Baseline: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` passed.
- Audit scope: repository tooling and all workspace crates.
- Findings: 4 delete, 2 consolidate, 1 fix-in-place, 3 product defects, 7 retain, 2 unresolved structure candidates.
- Deleted: five redundant tests across `aoa-construct`, `aoa-lint`, `aoa-policy`, and `aoa`; stronger behavioral and integration keepers failed under representative mutations before deletion.
- Coverage: workspace line coverage increased from 94.1911% to 94.2039% under `cargo llvm-cov --workspace --summary-only`.
- Rules added: none. Semantic test redundancy is a review judgment rather than a mechanically safe lint rule.
- Follow-ups: `aoa-dirn6` for malformed workspace member entries, `aoa-cmb1v` for forge/artifact mismatch, and `aoa-k1ads` for swallowed stderr failures.
- Retained for a later deletion cycle: resolved root artifact `FINDING-aoa-ksnq.md` and one consolidation candidate in `aoa-domain`.
- Next structure area: split `crates/aoa-audit/src/structure.rs` by analysis responsibility; it is the largest confirmed production module and needs an owner-focused boundary pass.
