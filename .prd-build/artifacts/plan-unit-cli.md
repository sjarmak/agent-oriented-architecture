# Plan — unit-cli

## Crate layout (`crates/aoa`)
- `Cargo.toml` — `[[bin]] name = "aoa"`; deps: clap (derive), serde_json, anyhow, the 6 workspace crates; dev-deps: assert_cmd, predicates, tempfile.
- `src/main.rs` — thin: parse `Cli`, dispatch, map `anyhow::Result<i32>` to `process::exit`.
- `src/cli.rs` — clap derive structs/enums for all subcommands + flags.
- `src/output.rs` — small helper: emit JSON (serde) vs human text uniformly; `render` trait-free helpers.
- `src/forge.rs` — R-silent fail-loud `compile_enforcement`.
- `src/commands/mod.rs` + one file per command:
  - `observe.rs`, `audit.rs`, `lint.rs`, `eval.rs` (validate-trace + compare), `falsify.rs`, `policy.rs`.
- `tests/cli.rs` — assert_cmd integration tests, all 10 criteria.
- `tests/fixtures/` — valid_trace.json, invalid_trace.json, baseline.json, migrated.json, falsify_input.json, plus a context md + changed/suppressed md.

## Dispatch contract
Each command returns `anyhow::Result<i32>` (exit code). main prints errors to stderr and exits with the code (or 1 on Err). No swallowed errors.

## Exit-code mapping
- validate-trace: 0 valid, non-zero (1) on Err.
- compare: 0 on success print, non-zero on GapError.
- audit: `exit_code(report, fail_on_tier1)`.
- falsify: writes file; exit 0 on success (verdict is data, not a failure). Err ⇒ non-zero.
- policy compile: 0 on known forge, non-zero on unknown (ForgeError).

## Tests → criteria
1. build: covered by `cargo build -p aoa` (criterion runs in test 10).
2. validate-trace valid exit 0 + per-type counts printed; invalid exit non-zero.
3. compare prints gap delta.
4. observe: temp git repo, `git status --porcelain` clean (only `.aoa/` which is ignored).
5. audit: human punch-list; `--json` parseable; `--fail-on tier1` exit non-zero only when Tier-1 present.
6. lint-context `--changed`: only changed file findings; suppressed file honored.
7. falsify: falsification.json has `verdict`.
8. forge unknown ⇒ non-zero + clear stderr.
9. dual register: `--json` parseable JSON, default human text — assert on audit + eval.
10. `cargo test -p aoa` green; `cargo build --workspace` green.
