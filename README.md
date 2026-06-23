# AOA Toolkit

Measure whether a repository's structure actually helps coding agents, on real agent traces, before changing a line of it.

Existing "Agent Readiness" graders score a repo by checking which files exist: is there an `AGENTS.md`, a `CODEOWNERS`, a test directory. None of them runs an agent. The AOA Toolkit runs the agent against contamination-free tasks, parses the resulting tool-call stream into a typed trace, and computes four locality metrics from it, each one conditioned on whether the agent actually solved a held-out version of the task. A checklist score that doesn't predict held-out success is the thing this instrument is built to expose.

"AOA" is Agent-Oriented Architecture: a repository and workflow design style for codebases edited by both humans and coding agents, where the correct place to read, the safe place to write, and the correct way to verify are made explicit to the agent. The full evidence base and pattern catalog live in [`report.md`](./report.md); the scope decisions and risk register live in [`prd_aoa_toolkit.md`](./prd_aoa_toolkit.md) and [`premortem_aoa_toolkit.md`](./premortem_aoa_toolkit.md).

## The instrument ships before the transforms

The toolkit is split into two waves by a single gate, because it tests rather than assumes the central claim that repository structure is the right layer to improve agent behavior.

**Wave 0 is read-only and ships first.** `observe` installs trace telemetry without touching a tracked file. `audit` reads the repo and returns a ranked, tiered punch-list with a measured cost per item. `lint-context` enforces a token budget over the transitive closure of context files. `eval` post-processes an agent run into the four metrics. Nothing here writes to the repository under test, and Wave 0 is independently useful: point it at a repo that scores well on a static grader and the score fails to predict held-out success.

**Wave 1 transforms the repo, and only runs once R0 says it should.** `aoa falsify` is itself an eval on the Wave-0 instrument. It compares the held-out improvement from migrating the repo (fixed harness) against the improvement from swapping the harness (fixed repo), across at least five calibrated repos, and emits one of three verdicts: `proceed`, `pivot`, or `inconclusive`. A `proceed` requires repo-delta to beat harness-delta on a strict majority, stable across repeated runs, invariant across scoring conventions, and backed by a power analysis. On a tie the default is `pivot`: the repo layer has the highest reversal cost, so ambiguity favors not touching it. If the verdict reads `pivot`, the migrator never ships and the product is the Wave-0 instrument.

That gate is the whole architecture. Everything in Wave 0 exists to make R0 a measurement rather than an opinion, and the [R0 runbook](./docs/r0_runbook.md) documents how to run it as a codeprobe experiment with paired config arms.

## Every metric reduces to an ordered span stream

The atomic unit is a trace: an ordered, gold-anchored stream of tool-call events emitted as OpenTelemetry-style spans. The schema defines eight span types, covering the agent's whole loop from search to abstain: `retrieval.search`, `file.read`, `symbol.lookup`, `write.attempt`, `write.blocked`, `test.run`, `gateway.invoke`, and `abstain`. A trace either validates against the published schema with correctly ordered spans or it fails loudly; reconstructed spans inferred from logs are tagged and excluded from the metrics that need native instrumentation.

```bash
aoa eval validate-trace path/to/trace.json   # prints per-type span counts, exits non-zero if invalid
```

## Four metrics, all conditioned on held-out success

`aoa eval run` turns a codeprobe run into per-task metric records. The four metrics are deliberately the surface signature of good agent behavior, and each is reported against the held-out outcome rather than the visible test pass, because visible-pass-plus-small-patch is also the signature of reward hacking.

- **Retrieval locality:** tool-calls-to-first-relevant-artifact, Recall@k, and MRR, with the gold set anchored to base-repo symbols through `transform-map.json`.
- **Edit locality:** patch inflation measured against both the intersection floor and the union ceiling of accepted solutions, so an underdetermined ranking stays visible instead of being hidden behind a single number.
- **Invariant discoverability:** whether the agent had pre-write access to the task's invariants.
- **Mutation surface:** writable files reachable in the SCIP graph at depth ≤ k, with `k` and `over_approximation: true` emitted in the record rather than pretending the undecidable slice was computed exactly.

The primary eval is the reward-hacking gap (`aoa eval compare baseline migrated`): the spread between visible and held-out success. A migration earns the label "good" only if it holds or reduces that gap while improving held-out pass rate. Two guards protect the gap itself. `aoa eval r0b` runs a leakage canary over a baseline and a migrated run and fails when held-out pass rate rises without the visible rate moving; toolkit-side synthesis of held-out tests from visible specs is forbidden. And `aoa gap` surfaces the construct-validity determination from R9c: no metric may gate a feature until a correlation report ties it to an external outcome, so until that report exists the metric is advisory, not gating.

## codeprobe supplies the tasks and runs the agent

The toolkit is the process layer on top of [codeprobe](https://github.com/sjarmak/codeprobe), a separate Python project that does the work the trace layer builds on: it mines contamination-free tasks from a repository's own history (which is what gives the held-out leg its integrity by construction), orchestrates the agent through `claude -p --output-format stream-json --verbose`, scores outcomes with a consensus-plus-AST oracle, and runs the baseline-versus-config experiments that the R0 comparison consumes. AOA reads codeprobe's per-trial transcript, parses it into a native eight-span trace, and computes the metrics, the budget gate, and the falsification verdict that codeprobe alone does not produce.

`aoa-bench` is the loader that turns codeprobe task directories into AOA task inputs. `aoa eval experiment` reads a manifest of paired repo-arm and harness-arm run directories and builds the `FalsifyInput` JSON that `aoa falsify` consumes.

## Commands

```text
aoa observe                       Install zero-write trace telemetry under .aoa/
aoa audit [--fail-on tier1]       Ranked, tiered, read-only audit punch-list
aoa lint-context [--changed ...]  Token-budget closure check over context files
aoa eval validate-trace <file>    Validate a trace; print span counts per type
aoa eval run --codeprobe-run DIR  Post-process a codeprobe run into metric records
aoa eval compare BASE MIGRATED    Print the reward-hacking gap delta
aoa eval r0b --baseline --migrated --tasks    Held-out integrity leakage canary
aoa eval experiment --manifest --tasks        Build the R0 falsification input
aoa gap                           Which metrics may gate a decision vs. advisory-only
aoa falsify --repos INPUT         Run the wrong-layer gate; write falsification.json
aoa migrate [--apply|--rollback]  Code-layer repo migration (dry-run by default)
aoa policy compile --forge NAME   Compile the enforcement plane (fails loud on unknown forge)
```

Every subcommand that produces a report takes `--json` for the structured rendering an agent can act on, alongside the colorized human output. `aoa migrate` is a dry-run preview unless given `--apply`, applies each fix as one revertable commit, and undoes the recorded manifest with `--rollback`.

## Build

A Rust workspace, edition 2021, pinned to Rust 1.94, MIT-licensed.

```bash
cargo build --workspace
cargo test  --workspace   # 312 tests
```

## Layout

The workspace is split so each crate owns one reason to change.

| Crate | Responsibility |
|-------|----------------|
| `aoa` | The CLI: wires each subcommand to its library crate, with fail-loud forge adapters and dual-register output. |
| `aoa-trace` | The eight-span trace substrate and its schema. |
| `aoa-codeprobe-shim` | Parses a codeprobe `stream-json` transcript into a native trace, preserving tool-call order and targets. |
| `aoa-bench` | Loads codeprobe-mined task directories into AOA task inputs. |
| `aoa-metrics` | The four metrics and the `SymbolGraph` they read. |
| `aoa-scip-graph` | Indexes a repo into a `SymbolGraph`: SCIP (high-confidence), AST (best-effort), or degraded, with R15 confidence tiering. |
| `aoa-budget` | The context-budget gate: transitive closure, dual tokenizer, ceiling enforcement. |
| `aoa-lint` | Config-smell detectors mapped to the 2606.15828 taxonomy, composed with the budget closure. |
| `aoa-gap` | The reward-hacking gap, the R0b held-out canary, and R9c construct-validity gating. |
| `aoa-falsify` | The R0 wrong-layer gate with its robustness, abstention, and monitoring hardening. |
| `aoa-audit` | Zero-write telemetry install and the read-only tiered audit. |
| `aoa-migrate` | Reproducible, oracle-blind, code-layer migrations with dry-run preview and rollback. |

## Status

Wave 0 is built and tested. The R0 machinery (the `experiment` builder, `falsify`, and the runbook) is in place; running it at scale on five-plus calibrated repos with live, non-deterministic agent runs is the next milestone, and `inconclusive` is an expected outcome there rather than a defect. Issue tracking runs on [beads](https://github.com/gastownhall/beads); `bd ready` shows what is open.
</content>
</invoke>
