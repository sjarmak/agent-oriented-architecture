# Research — unit-context-lint (R13)

## aoa-budget public API (confirmed from src)

- `resolve_closure(root: &Path) -> Result<Closure, BudgetError>`
  - `Closure { root: PathBuf, files: Vec<ContextFile> }`, `Closure::paths() -> BTreeSet<PathBuf>`
  - `ContextFile { path: PathBuf, text: String }`
- `count_budget(closure: &Closure, target: &str, config: &Config) -> Result<BudgetReport, BudgetError>`
  - NOTE: `count_budget` takes THREE args (`closure`, `target`, `config`), not two. The unit brief said `count_budget(&Closure, target)`; actual signature requires a `&Config`. We construct a default blocking config inside `lint_context`.
  - `Config::blocking(ceiling)` / `Config::warn_first(ceiling)`; fields `ceiling`, `warn_first`, `changed_files`.
  - `BudgetReport { o200k_tokens, target_tokens, gating_target_tokens, reference_encoding, target_model, ceiling, verdict, files: Vec<FileBudget> }` — `Serialize`/`Deserialize`, `Clone`.
- `BudgetError` (thiserror): `Io{path,source}`, `UnknownTargetTokenizer{name,supported}`, `FixFailed{...}`.
- Supported target tokenizer names: `o200k_base`, `cl100k_base`, plus model aliases (gpt-4o, gpt-4, ...). Unknown names error loudly.

## Closure file model

- Each closure file already carries its full `text` — detectors operate on `ContextFile.text` directly, no re-read needed. We reuse the closure to know WHICH files to lint (criterion 2 reuse).
- Root is always `files[0]`. Paths are lexically normalized.

## Reference / reuse decisions

- `lint_context` runs `resolve_closure(root)` then `count_budget(&closure, target, &Config::blocking(LINT_CEILING))` and embeds the resulting `BudgetReport` in `LintReport.budget`. This is the "compose closure results" requirement.
- Ceiling for the embedded budget is a non-gating concern for lint (we don't fail on it), so we pick a large default ceiling; the budget section is informational here.

## 2606.15828 config-smell taxonomy mapping (mechanical/structural only — ZFC)

We expose `pub enum SmellCategory` with stable id strings, each documented as a catalog category:
- `Contradiction` (id `contradiction`) — structurally contradictory directives (e.g. a line saying "always X" and another "never X" on the same token).
- `Duplication` (id `duplication`) — duplicate headings within a file (redundant structure).
- `Verbosity` (id `verbosity`) — oversized section (heading block exceeding a line threshold).
- `StaleReference` (id `stale_reference`) — markdown link to a local file that does not exist on disk (dead link).
- `OverBroadGlob` (id `overbroad_glob`) — a bare `**` / `**/*` glob directive (over-broad scope).

All detectors are mechanical/deterministic structural checks. No LLM.

## Test plan (covers 4 criteria)

1. Fixture AGENTS.md/rules tree triggers >=3 distinct smell types; assert each finding carries a mapped category id. (criteria 1 + 3 fields)
2. `LintReport` has both `budget` (BudgetReport) and `findings` populated. (criterion 2)
3. A single finding asserts `file`, `message`, `category` all present. (criterion 3)
4. `cargo test -p aoa-lint` green. (criterion 4)
