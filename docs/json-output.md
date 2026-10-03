# Dual-register output (R17): the stable `--json` schema

Every operator-facing pillar command renders its result in two registers from
the **same in-memory report value**:

- **human** (default): colorized/plain terminal text via `render_human()` or an
  equivalent renderer in `crates/aoa/src/commands/`,
- **agent** (`--json`): pretty-printed JSON of that same value on stdout.

Because both registers serialize one value, a finding present in one register
is present in the other by construction. Exit codes are identical across
registers; JSON is always written to stdout, human diagnostics for the
pre-commit guard stay on stderr (what pre-commit surfaces to the committer).

## Stability contract

The top-level field names below are stable: they are read by agents for
self-remediation. New fields may be added; existing fields are not renamed or
removed without a major version note. Field types are given as JSON types.

## Per-command schemas

| Command | Top-level JSON fields | Source of truth |
|---|---|---|
| `aoa audit --json` | `items` (array of punch items), `behavioral_signal`, `insufficient_data?`, `live_observations?` (per-session `measured` metrics or stable typed `excluded` reason), `graph_coverage` (object: `indexed`, `unindexed`, each a language-to-file-count map for the best-effort symbol graph), `graph_degrade_reason?` (string, present when fewer than a tenth of the source files were indexed and live sessions are excluded as `symbol_graph_missing`) | `aoa_audit::AuditReport`; graph fields from `GraphNotice` in `crates/aoa/src/commands/pipeline.rs` |
| `aoa recommend --json` | `items` (array: finding + fix + mode join), `actionable_now` (number), `advisory_only` (number), `graph_coverage` (object: `indexed`, `unindexed`, each a language-to-file-count map for the best-effort symbol graph), `graph_degrade_reason?` (string, present when fewer than a tenth of the source files were indexed and live sessions are excluded as `symbol_graph_missing`) | `aoa_recommend::RecommendationReport`; graph fields from `GraphNotice` in `crates/aoa/src/commands/pipeline.rs` |
| `aoa gap --json` | `data_source` (string), `thresholds` (object), `metrics` (array of per-metric classifications) | `aoa_construct::ConstructValidityReport` |
| `aoa migrate --json` | dry-run: `grounding_navigability_sites`, `fix_ids`, `changes`, `eligibility_notes`, `provenance`, `fix_reports`; `--apply`: `fixes_applied`, `files_written`, `navigability_sites_remaining`, `manifest_path`, `eligibility_notes`, `provenance`, `fix_reports`; `--rollback`: `files_reverted`. `fix_reports` has one entry per selected fix, including fixes that planned nothing: `fix_id`, `ran` (false when the repo lacks the fix's root marker), `examined` and `examined_unit` (package roots for the navigability fix; for a dead-import fix the source files of its language in the isolated copy, which `cargo` and `ruff` may narrow further and ESLint lints in full), `changes`, and `no_change_reason` (a string when `changes` is 0, otherwise null) | `MigrateView` in `crates/aoa/src/commands/migrate.rs` |
| `aoa lint-context --json` | `roots` (string array: every context file whose closure was linted), `budget` (object: `tokenizer`, `reference_encoding`, `ceiling`, `distinct_files`, `target_tokens`, `o200k_tokens` over the distinct files of every closure, and `closures`, one entry per root with `root`, `target_tokens`, `o200k_tokens`, `gating_target_tokens`, `over_ceiling_by` (0 unless the closure's gating tokens exceed `--ceiling`) and `files` (array: `file`, `target_tokens`, `o200k_tokens`, `gating`; a suppressed file is not gating); a file two roots reach counts in both closures and once in the totals; `--changed` does not narrow it; a breach does not change the exit status), `findings` (array: `file`, `category`, `message`), `suppressed` (array: `file`, `reason`) | `LintView` in `crates/aoa/src/commands/lint.rs` |
| `aoa falsify --json` | the `falsification.json` document: `verdict`, `precondition_unmet?`, `repo_delta?`, `harness_delta?`, `eligible_repos?`, `excluded_repos?`, `conventions_tried?`, `notes`, `bias_warnings?`, `bias_gate_invalidating` | `FalsificationOutput` in `crates/aoa/src/commands/falsify.rs` |
| `aoa eval validate-trace --json` | `total` (number), `has_reconstructed` (bool), `counts` (array: `span_type`, `count`) | `TraceView` in `crates/aoa/src/commands/eval.rs` |
| `aoa eval compare --json` | `gap_delta`, `held_out_delta` (numbers), `label` (string) | `aoa_gap` compare outcome via `crates/aoa/src/commands/eval.rs` |
| `aoa eval run --json` | per-task AOA metric records | `crates/aoa/src/commands/eval_run.rs` |
| `aoa eval r0b --json` | leakage-canary composition report | `crates/aoa/src/commands/r0b.rs` |
| `aoa eval experiment --json` | `FalsifyInput` build report, including observation sidecar path/SHA-256/count/IDs and per-repo `candidate_pairs`, `identical_pairs`, and `pair_yield` | `aoa_falsify_build::BuildReport`; `aoa_bench::MeasurementObservationV1` |
| `aoa init --json` | `mode` (string), `template_version` (number), `written`, `skipped`, `review` (string arrays) | `InitView` in `crates/aoa/src/commands/init.rs` |
| `aoa observe --json` | `traces_dir` (string), `gitignore` (string), `enforce_settings` (string or null) | `ObserveView` in `crates/aoa/src/commands/observe.rs` |
| `aoa policy compile --json` | `planes_written` (string array of artifact paths) | `CompileView` in `crates/aoa/src/commands/policy.rs` |
| `aoa policy guard-staged --json` | `blocked` (string array of protected staged files; exit 1 when non-empty) | `GuardStagedView` in `crates/aoa/src/commands/policy.rs` |
| `aoa policy infer-owners --json` | `codeowners_path` (string), `entries` (array: `pattern`, `owner`, `owned_lines`, `total_lines`), `proposal` (string), `diff` (string), `written` (bool); `proposal` and `diff` are empty when `entries` is empty | `InferOwnersView` in `crates/aoa/src/commands/policy.rs` |

### Live-observation trust boundary

Files under an audited repository’s `.aoa/traces/` are untrusted measurement
candidates. A valid envelope, monotonic sequence, or content hash establishes
structure or byte identity—not origin. The CLI therefore never derives
same-task metric context from the audited repository: without context supplied
by a trusted caller, every planted trace remains `excluded` with
`task_context_missing` and contributes zero to `behavioral_signal`.

Repository-local signing would not improve this boundary because an actor able
to plant traces can also replace a key stored beside them. Any future feature
that imports task context or promotes live correlations to `gating` must first
define an external trust root and authenticate both the trace and its same-task
context.

## Exemption

`aoa enforce record` / `aoa enforce check` are not operator-facing commands:
they are the runtime hook entry points invoked by Claude Code, and their
stdin/stdout wire format is the hook protocol itself. They carry no human
register by design.
