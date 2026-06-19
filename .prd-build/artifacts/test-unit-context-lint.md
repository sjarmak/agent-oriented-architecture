# Test Results — unit-context-lint (R13)

## Commands

- `cargo test -p aoa-lint` — 7 passed, 0 failed
- `cargo build --workspace` — clean
- `cargo clippy -p aoa-lint` — no warnings
- `cargo fmt -p aoa-lint` — applied

## Acceptance criteria coverage

1. **Smells + catalog mapping (>=3 distinct)** — `detects_distinct_smell_categories_mapped_to_catalog`
   asserts >=3 distinct `SmellCategory` ids; `fixture_triggers_each_catalog_category` confirms all
   5 categories (contradiction, duplication, verbosity, stale_reference, overbroad_glob) fire on the
   fixture AGENTS.md/rules tree and that ids are stable.
2. **Composition of budget + findings in one report** — `report_composes_budget_and_findings`
   asserts both `report.budget` (closure file set >=2, target_tokens > 0, target_model) and
   `report.findings` are populated. `linted_files_come_from_budget_closure` confirms reuse: every
   finding file is in the budget closure. `report_serializes_to_json` confirms single structured
   report round-trips.
3. **Finding has path + message + category** — `finding_has_path_message_and_category` asserts all
   three fields present and non-empty.
4. **`cargo test -p aoa-lint` green** — 7/7 pass.

Extra: `unknown_tokenizer_errors` confirms loud failure on bad target tokenizer (no silent default).

## Test output

```
running 7 tests
test unknown_tokenizer_errors ... ok
test report_composes_budget_and_findings ... ok
test finding_has_path_message_and_category ... ok
test report_serializes_to_json ... ok
test linted_files_come_from_budget_closure ... ok
test detects_distinct_smell_categories_mapped_to_catalog ... ok
test fixture_triggers_each_catalog_category ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
