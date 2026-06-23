# AOA Toolkit

Measure how well a repository actually works for AI coding agents, by running agents against it and reading what they do.

Most "is my repo ready for AI agents?" tools check boxes: is there an `AGENTS.md`, a `CODEOWNERS`, a test directory. None of them runs an agent, so none can tell you whether the structure they reward actually helps. The AOA Toolkit runs a coding agent against real tasks drawn from your repository's own history, records every search, file read, and edit the agent makes, and scores how quickly it found the right code and how cleanly it changed it. The numbers come from what the agent did, not from which files happen to exist.

Agent-Oriented Architecture (AOA) is the underlying idea: a way of organizing a codebase so the correct place to read, the safe place to write, and the correct way to verify are obvious to an automated agent, not just to a human who already knows the project. This toolkit is the instrument for measuring whether a given repository has those properties, and for changing the repository only where the measurement says it will help.

## What you get

Point the toolkit at a repository and a recorded agent run, and it answers four questions, each one about real agent behavior rather than file presence.

- **Did the agent find the right code fast?** Retrieval locality counts how many tool calls it took to reach the first relevant file, plus recall and ranking quality against the known-correct set of files for the task.
- **Did it change only what it needed to?** Edit locality compares the size of the agent's patch against the smallest and largest accepted human solutions, so an over-broad change stands out.
- **Could it discover the rules before writing?** Invariant discoverability checks whether the constraints a task depends on were reachable before the agent started editing.
- **How much could it have broken?** Mutation surface counts the files an edit could reach through the symbol graph, bounding the blast radius of a change.

Every one of these is scored against a hidden version of the task the agent never saw, not against the visible tests. An agent that aces the visible tests while failing the hidden ones has gamed the metric, and the gap between the two is the toolkit's primary signal.

## Quickstart

```bash
cargo build --workspace

# 1. Install trace logging. Writes nothing the agent can see beyond an ignored .aoa/ dir.
aoa observe

# 2. Read the repo and print a ranked, tiered punch-list with a measured cost per item.
aoa audit

# 3. Check that your agent context files (AGENTS.md and what they reference) fit a token budget.
aoa lint-context --changed AGENTS.md

# 4. Turn a recorded agent run into per-task metrics.
aoa eval run --codeprobe-run path/to/run --tasks path/to/tasks
```

`audit` and `lint-context` are read-only. Nothing in the quickstart modifies a tracked file in the repository under test.

## Measure first, change later

The toolkit is split so that the read-only measurement ships and stands on its own, and the parts that rewrite your repository stay behind a gate until the measurement proves they earn their place.

The read-only side is `observe`, `audit`, `lint-context`, and `eval`. It installs telemetry, reports what an agent struggles with, enforces a context budget, and computes the metrics. You can run all of it against any repository without it touching a single tracked file.

The repository-changing side is `migrate`, which applies code-layer cleanups (dead-import removal and similar structural fixes) as reversible, one-commit-per-fix changes, dry-run by default. Before that side is worth trusting on a given codebase, `aoa falsify` runs the deciding experiment: it compares how much a repository migration improves agent success against how much simply swapping the agent's harness improves it, across several repositories. If swapping the harness helps as much as migrating the repo, then the repo was the wrong thing to change, and the toolkit says so. The verdict is one of `proceed`, `pivot`, or `inconclusive`, and a tie defaults to leaving the repository alone, because the repository is the most expensive layer to get wrong.

That gate is the point of the whole design. The read-only instrument exists, in part, to make the change-the-repo decision a measurement instead of an opinion.

## How a run is scored

The unit of measurement is a trace: the ordered stream of actions an agent took, captured as structured spans covering its whole loop, from searching and reading to writing, running tests, and abstaining. A trace either validates against the published schema with its spans in the right order, or it fails loudly.

```bash
aoa eval validate-trace path/to/trace.json   # prints span counts per type; non-zero exit if invalid
aoa eval compare baseline.json migrated.json # prints the visible-vs-hidden success gap delta
```

The toolkit does not run the agent itself. That job belongs to [codeprobe](https://github.com/sjarmak/codeprobe), a companion project that mines contamination-free tasks from a repository's history (which is what makes the hidden test set genuinely hidden), drives the agent, and scores the outcomes. The AOA Toolkit reads codeprobe's transcript of each run, reconstructs the trace, and computes everything above. codeprobe supplies the tasks and the runs; this toolkit supplies the trace-level metrics and the gate.

## Commands

```text
aoa observe                       Install zero-write trace telemetry under .aoa/
aoa audit [--fail-on tier1]       Ranked, tiered, read-only audit punch-list
aoa lint-context [--changed ...]  Token-budget check over your agent context files
aoa eval validate-trace <file>    Validate a trace; print span counts per type
aoa eval run --codeprobe-run DIR  Turn a codeprobe run into per-task metric records
aoa eval compare BASE MIGRATED    Print the visible-vs-hidden success gap delta
aoa gap                           Which metrics are trustworthy enough to gate a decision
aoa falsify --repos INPUT         Run the deciding experiment; write falsification.json
aoa migrate [--apply|--rollback]  Code-layer repo cleanup (dry-run by default)
aoa policy compile --forge NAME   Compile enforcement config for a CI forge
```

Every reporting command takes `--json` for a structured rendering alongside the human-readable output. `aoa migrate` previews diffs and writes nothing unless given `--apply`, then applies each fix as its own revertable commit and undoes the last run with `--rollback`. Two further `eval` subcommands, `r0b` and `experiment`, build the inputs the falsification gate consumes; see `docs/r0_runbook.md`.

## Install

A Rust workspace, edition 2021, pinned to Rust 1.94, MIT-licensed.

```bash
cargo build --workspace
cargo test  --workspace
```

The TypeScript migration adapter runs a pinned, hermetic ESLint. That install is not committed; regenerate it once with `npm ci` in `crates/aoa-migrate/assets/eslint/`, and the adapter (and its tests) will find it. Without it, the adapter fails with a clear message rather than producing an empty result.

## Layout

The workspace is split so each crate owns one responsibility.

| Crate | Responsibility |
|-------|----------------|
| `aoa` | The CLI: wires each subcommand to its library crate, with human and JSON output. |
| `aoa-trace` | The span-based trace format and its schema. |
| `aoa-codeprobe-shim` | Parses a codeprobe transcript into a trace, preserving tool-call order and targets. |
| `aoa-bench` | Loads codeprobe-mined task directories into toolkit task inputs. |
| `aoa-metrics` | The four metrics and the symbol graph they read. |
| `aoa-scip-graph` | Indexes a repo into a symbol graph (SCIP, with an AST fallback) and labels its confidence. |
| `aoa-budget` | The context-budget check: transitive closure of context files, real tokenizer, ceiling. |
| `aoa-lint` | Config-file smell detectors composed with the budget closure. |
| `aoa-gap` | The visible-vs-hidden success gap, its leakage guard, and the metric-trust determination. |
| `aoa-falsify` | The deciding repo-vs-harness experiment and its robustness checks. |
| `aoa-audit` | The zero-write telemetry install and the read-only audit. |
| `aoa-migrate` | Reversible, code-layer migrations with dry-run preview and rollback. |

## Status

The read-only instrument is built and tested. The machinery for the deciding experiment is in place; running it at scale across several real repositories with live agents is the next milestone, and an `inconclusive` verdict is an expected result there, not a defect. This is early software at version 0.1.0; the metric definitions and the gate are the parts most likely to move as that experiment runs.
</content>
</invoke>
