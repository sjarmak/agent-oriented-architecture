# Test Results — unit-falsify

## Commands
- `cargo test -p aoa-falsify` => 10 passed, 0 failed.
- `cargo build --workspace` => Finished, all crates compile.
- `cargo clippy -p aoa-falsify --all-targets -- -D warnings` => clean (0 warnings).
- `cargo fmt -p aoa-falsify` => applied.

## Acceptance criteria coverage
| # | Criterion | Test |
|---|-----------|------|
| 1 | Emits falsification.json with repo_delta, harness_delta, verdict ∈ {proceed,pivot,inconclusive} | `emits_falsification_json_with_required_fields` (fixture-driven, serializes + round-trips) |
| 2 | proceed iff repo-delta >= harness-delta on MAJORITY of >=5; exact tie => pivot | `majority_rule_table_driven_including_tie` (4 cases incl. 3-for/3-against tie => pivot) |
| 3 | Only identical-pair tasks contribute | `only_identical_pair_tasks_contribute` (non-paired harness-win task excluded; deltas 1.0/0.0) |
| 4 | Determinism gate: unstable across K>=3 runs => inconclusive, never proceed | `determinism_gate_unstable_runs_inconclusive` (flipper repo swings run 0 vs run 1) |
| 5 | Convention-invariance: proceed flipping under any admissible convention => inconclusive | `convention_invariance_flip_downgrades_to_inconclusive` (alternative_metric_weights flips the vote) |
| 6 | Ineligible (low-conf / reconstructed / uncalibrated) repos excluded from voting | `ineligible_repos_excluded_from_voting` (3 ineligible "proceed" votes ignored; eligible majority pivots) |
| 7 | Power precondition: holdout below threshold => inconclusive | `power_precondition_small_holdout_inconclusive` + `power_precondition_effect_size_inconclusive` |
| 8 | inconclusive never silently converted to pivot; preserved verbatim in json | `inconclusive_preserved_verbatim_in_json` (json field == "inconclusive", != "pivot") |
| 9 | cargo test -p aoa-falsify passes with 0 failures | verified |

Extra: `too_few_repos_is_error` guards the structural <5-repo path (returns `FalsifyError::TooFewRepos`, not a verdict).

## Design notes
- Power effect-size uses mean ABSOLUTE |repo_delta - harness_delta| (magnitude of evidence), so a genuine pivot is not mistaken for "too weak to call". Holdout-size and effect-magnitude both gate any significant verdict.
- Verdict downgrades (proceed -> inconclusive) are data, not errors. `thiserror` (`FalsifyError`) covers only structural input failures.
- Admissible conventions are emitted in `conventions_tried`; precondition reasoning is emitted in `notes` (ZFC: policy as data).
- No RNG; determinism modeled as caller-supplied fixed-seed run snapshots compared for verdict stability.
