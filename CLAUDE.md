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
gc-outcome-close <id> --producer formula-step --reason "Done: <summary>"  # Complete formula work (atomic)
bd close <id>         # Complete work (non-formula work only)
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Formula-dispatched work is NOT complete until `gc-outcome-close` succeeds; pushing follows this rig's publication policy.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Close finished work atomically** - Formula work: `gc-outcome-close <id> --producer formula-step --reason "Done: <summary>"` (add `--passing-verdict evidence.reviewer_verdict` when `gc.review_gate=pass`); plain `bd close <id>` only for work never dispatched through a formula. Update in-progress items
4. **Publish per rig policy** - Only when this rig's publication policy authorizes pushing your work:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed; closes accepted (not reopened by the close gate); pushed when publication applies
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Formula-dispatched work is NOT complete until `gc-outcome-close` succeeds - never substitute plain `bd close`, and never write `gc.outcome=pass` or a passing review-verdict marker before it
- Push ONLY under this rig's publication policy; when publication applies, do not stop before the push succeeds - resolve failures and retry
- If a close is reopened within an hour, the close gate rejected it - fix the required metadata while the bead is open; do not force a re-close
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
- Capture and inputs: `aoa-path-trust`, `aoa-trace`, `aoa-codeprobe-shim`,
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

`aoa-path-trust` is a separate crate, not a module of `aoa-trace`, because the
two change for different reasons. It owns the filesystem and git trust
primitives: the component validator, the symlink-refusing join, descriptor-based
directory acquisition, the repository-root resolver and the environment a git
subprocess may inherit. `aoa-trace` owns the trace wire format. While the
primitives lived in `aoa-trace`, every hardening of the trust boundary was a
change to the trace crate, and a `rustix` type in their public surface tied the
trace crate's API to that dependency's major version (aoa-9pylp). It has no
internal dependency and takes no trace or measurement type, and it is not
`aoa-domain`, which holds vocabulary only.
[docs/adr/0008-path-trust-crate.md](docs/adr/0008-path-trust-crate.md) records
the split and what the crate may acquire.

`aoa-domain` is the bottom of the stack and holds only the vocabulary every
other layer needs to name a held-out subject: `SubjectKey`, `ExposureStatus`,
`HeldOutProvenance`, `RunResult`. It exists so that mining a task does not
require depending on the gate that scores it — while that vocabulary lived in
`aoa-gap`, `aoa-bench` had to reach up into measurement to say what a subject
was (aoa-ynqcn). Nothing that makes a judgment belongs here, and it must never
acquire an internal dependency; there is nothing below it to depend on.

The intended dependency direction is domain → inputs → measurement → decisions
→ controlled changes and enforcement → CLI: the bullets above are in that
order, and every layer the list names appears in it. Controlled changes sit
after decisions because a fix is applied to a finding somebody else decided to
raise — `aoa-migrate` reads `aoa-audit`'s findings, and a decisions crate that
reaches back the other way is the inversion aoa-4s25v removed.
`crates/aoa/tests/architecture_doc.rs` enforces the direction against each
crate's Cargo manifest: a dependency pointing at a later layer fails the
workspace tests unless it is in that file's explicit, bead-tracked exception
list. Library crates must not depend on CLI concerns.
Human and JSON output are dual registers of the same result, not separate
implementations.

`architecture/model.c4` draws the same system and is published as a website. It
is a maintained contract, not a snapshot: every arrow between two crates is a
real production dependency, and every production dependency between two library
crates is an arrow, so adding a crate or changing an edge means editing the
model. `crates/aoa/tests/architecture_model.rs` fails the workspace tests
otherwise, and
[docs/adr/0006-architecture-model-conformance.md](docs/adr/0006-architecture-model-conformance.md)
records the rule and what it deliberately leaves unenforced. The model's
containers group by concern and do not mirror the layers above — the layer list
here is the one that decides where new code goes.

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
- A test whose precondition CI can satisfy skips with a printed notice, and CI
  installs whatever it needs; one whose precondition CI can never satisfy is
  `#[ignore]`d, because libtest captures a passing test's output and a notice
  would report `ok` for a run that checked nothing.
  [docs/adr/0004-environment-dependent-test-skips.md](docs/adr/0004-environment-dependent-test-skips.md)
  records the rule; `crates/aoa/tests/environment_dependent_skips.rs` counts the
  sites of each kind and fails on any it has not classified, on a registry that
  stops matching the record, and on CI dropping an install the printed-notice
  sites depend on. It reaches the two idioms spelled as the record spells them,
  not a test that returns early announcing nothing.
- `anyhow` is the CLI's error type and only the CLI's. Every library crate
  states a `thiserror` enum, including for errors it never means a caller to
  match on — a `Result` alias hides the type but does not make it private, and
  a library that reaches for `anyhow` internally ends up returning it. The last
  two exceptions were retired by aoa-o3ww7 (`aoa-falsify-build`) and aoa-wp6g7
  (`aoa-enforce`); `crates/aoa-falsify-build/src/error.rs` and
  `crates/aoa-enforce/src/live_log/error.rs` are the worked examples, each
  recording which contexts it merged into one variant and why.
- Preserve unrelated user changes and use `bd` for every unit of tracked work.
