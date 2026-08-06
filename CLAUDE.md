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

- Domain kernel: `aoa-domain`.
- Capture and inputs: `aoa-trace`, `aoa-codeprobe-shim`,
  `aoa-observe-shim`, `aoa-bench`.
- Measurement: `aoa-metrics`, `aoa-scip-graph`, `aoa-budget`, `aoa-lint`,
  `aoa-gap`, `aoa-construct`, `aoa-corpus`.
- Decisions and reporting: `aoa-audit`, `aoa-recommend`, `aoa-falsify`,
  `aoa-falsify-build`.
- Controlled changes and enforcement: `aoa-policy`, `aoa-enforce`,
  `aoa-migrate`.

Every library crate gets exactly one layer here, and that assignment is the
answer to "where does this new code go". `crates/aoa/tests/architecture_doc.rs`
holds the list to it: a crate added under `crates/` with no layer, or a layer
naming a crate that no longer exists, fails the workspace tests.

`aoa-corpus` is a measurement crate, not an input one. It mines revert history
and scores the Factory checkbox rubric, but it does so to join those outcomes
onto `aoa-construct`'s classification — that join is its reason to change, and
it is why the crate depends on `aoa-construct` rather than the reverse.
`aoa-falsify-build` is the assembly crate that joins mined inputs and
measurements into the evidence `aoa-falsify` scores; it sits with decisions
because it depends on `aoa-falsify` for the shape it produces.

`aoa-domain` is the bottom of the stack and holds only the vocabulary every
other layer needs to name a held-out subject: `SubjectKey`, `ExposureStatus`,
`HeldOutProvenance`, `RunResult`. It exists so that mining a task does not
require depending on the gate that scores it — while that vocabulary lived in
`aoa-gap`, `aoa-bench` had to reach up into measurement to say what a subject
was (aoa-ynqcn). Nothing that makes a judgment belongs here, and it must never
acquire an internal dependency; there is nothing below it to depend on.

The intended dependency direction is domain → inputs → measurement → decisions
→ CLI, and `crates/aoa/tests/architecture_doc.rs` enforces it against each
crate's Cargo manifest: a dependency pointing at a later layer fails the
workspace tests unless it is in that file's explicit, bead-tracked exception
list. Library crates must not depend on CLI concerns. Human and JSON output are
dual registers of the same result, not separate implementations.

## Decision records

Standing decisions live in `docs/adr/`, indexed by
[docs/adr/README.md](docs/adr/README.md). Scan that index before proposing work:
a proposal that re-opens a recorded decision has to say which record it overturns
and why, and one that duplicates a record is already built. Citing the scan is
what the reinvention gate asks for, so the path has to keep resolving —
`crates/aoa/tests/decision_records.rs` fails the workspace tests if it stops, if
a record is added without an index row, or if the index links a file that does
not exist.

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
