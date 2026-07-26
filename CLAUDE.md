# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->


## Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Run a crate or test target while iterating, then run the workspace gates before
landing. The workspace requires Rust 1.94. The optional TypeScript migration
adapter is bootstrapped with `npm ci` in
`crates/aoa-migrate/assets/eslint/`; Rust tests degrade clearly when it is absent.

## Architecture Overview

AOA is a Rust workspace that consumes traces and outcomes produced by the
separate `codeprobe` project. The `aoa` crate is the CLI composition root; the
remaining crates are narrow libraries:

- Capture and inputs: `aoa-trace`, `aoa-codeprobe-shim`,
  `aoa-observe-shim`, `aoa-bench`, `aoa-corpus`.
- Measurement: `aoa-metrics`, `aoa-scip-graph`, `aoa-budget`, `aoa-lint`,
  `aoa-gap`, `aoa-construct`.
- Decisions and reporting: `aoa-audit`, `aoa-recommend`, `aoa-falsify`.
- Controlled changes and enforcement: `aoa-policy`, `aoa-enforce`,
  `aoa-migrate`.

The intended dependency direction is inputs → measurement → decisions → CLI.
Library crates must not depend on CLI concerns. Human and JSON output are dual
registers of the same result, not separate implementations.

## Conventions & Patterns

- Fail loudly on malformed or oversized measurement artifacts; missing optional
  evidence may degrade to an explicit unavailable/insufficient-data result.
- Keep read-only commands genuinely read-only. Repository-changing behavior is
  explicit, reviewable, and reversible.
- Treat held-out provenance and anti-leakage checks as load-bearing. Do not
  weaken them to make an experiment pass.
- State wire formats once in their owning crate and use `#[serde(deny_unknown_fields)]`
  at operator-authored input boundaries where schema drift must fail.
- Add regression tests at the public boundary that exposed a defect. Security
  filesystem tests must prove the planted target was not modified.
- Preserve unrelated user changes and use `bd` for every unit of tracked work.
