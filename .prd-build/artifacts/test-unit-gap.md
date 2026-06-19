# Test results — unit-gap

## `cargo test -p aoa-gap`
7 passed, 0 failed (tests/criteria.rs). Lib + doc tests: 0 tests, ok.

## `cargo build --workspace`
Finished clean. aoa-gap build: no warnings. `cargo clippy -p aoa-gap --all-targets`: clean.

## Acceptance criteria coverage
1. Gap + delta between two runs — `criterion_1_gap_and_delta` (visible/held-out rates, gap, held_out_delta, gap_delta).
2. `good` only when held-out improves AND gap holds-or-reduces; visible/locality-only -> NotGood — `criterion_2_label_table` (5 table cases incl. gap-widens and held-out-regresses).
3. Leakage canary fails when held-out rises while visible flat and a known item flips — `criterion_3_leakage_canary_fails`; honest gain not flagged — `criterion_3_no_false_positive`.
4. Synthesized-from-visible held-out rejected with Err in both compute_gap and compare — `criterion_4_synthesis_rejected`.
5. No native composed suite -> `GapOutcome::Unavailable`; compare refuses with `GapError::GapUnavailable` (either side absent) — `criterion_5_unavailable_refuses_label`.
6. Metric advisory without positive correlation; gating only with >=1 positive external outcome (revert/incident/review) — `criterion_6_construct_validity`.
7. Whole suite green — confirmed above.
