# Test results — unit-cli

## Commands
- `cargo build -p aoa` → green; single binary `target/debug/aoa` produced.
- `cargo build --workspace` → green.
- `cargo test -p aoa` → 18 passed, 0 failed (2 unit forge tests + 16 CLI integration tests).
- `cargo fmt --check -p aoa` → clean.
- `cargo clippy -p aoa --all-targets -- -D warnings` → clean.

## Acceptance criteria coverage
1. Binary builds, named `aoa` — `[[bin]] name = "aoa"`; verified by `cargo build -p aoa` + `Command::cargo_bin("aoa")` in every CLI test.
2. validate-trace valid exits 0 + per-type counts (`validate_trace_valid_prints_counts_and_exits_zero`); invalid exits non-zero (`validate_trace_invalid_exits_non_zero`).
3. compare prints gap delta (`compare_prints_gap_delta`, `compare_json_carries_gap_delta`).
4. observe leaves the working tree clean in a temp git repo (`observe_makes_no_tracked_changes`).
5. audit prints a tiered punch-list; `--json` structured; `--fail-on tier1` exits non-zero only with a Tier-1 gap (`audit_human_prints_punch_list`, `audit_json_is_parseable`, `audit_fail_on_tier1_exits_non_zero_when_tier1_present`, `audit_fail_on_tier1_exits_zero_without_tier1_gap`, `audit_without_fail_on_exits_zero_even_with_tier1_gap`).
6. lint-context `--changed` flags only changed files and honors the oversized-context suppression (`lint_context_changed_filters_and_honors_suppression`, `lint_context_human_renders_text`).
7. falsify writes falsification.json with a verdict field (`falsify_writes_verdict_file`).
8. R-silent: unknown forge fails loudly non-zero with clear stderr (`policy_compile_unknown_forge_fails_loudly`); known forge succeeds (`policy_compile_known_forge_succeeds`) + unit tests in `forge.rs`.
9. R17 dual register: `--json` parseable JSON and default human text for both audit and eval (`audit_json_is_parseable`, `validate_trace_json_is_parseable`, `compare_json_carries_gap_delta`, plus human-text assertions).
10. `cargo test -p aoa` 0 failures; `cargo build --workspace` green.

## Notes
- `o200k_base` chosen as the default lint tokenizer because it loads offline (tiktoken-rs bundles it), keeping CLI tests network-free.
- `--fail-on` constrained to `tier1` via clap `value_parser`; library `exit_code(report, fail_on_tier1)` is the single source of truth for the exit code.
- Forge adapter set is intentionally minimal (`github-actions`, `gitlab-ci`) — this is the R-silent fail-loud guarantee, not Wave-1 policy compilation.
