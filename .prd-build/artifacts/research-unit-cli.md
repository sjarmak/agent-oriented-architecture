# Research — unit-cli (aoa binary)

Exact public APIs read from each crate's source. The CLI adapts to these — no invented signatures.

## aoa-trace
- `validate_trace(path: &Path) -> Result<TraceReport, TraceError>` — reads + schema-validates + ordering-checks.
- `TraceReport`: `count(SpanType) -> usize`, `counts() -> &BTreeMap<SpanType, usize>`, `total() -> usize`, `has_reconstructed() -> bool`.
- `SpanType::as_str()`, `SpanType::ALL`. Trace wire format: `{ "spans": [ { "type", "source", "seq", "attributes" } ] }`.
- `TraceError` Display covers Read / Schema / OutOfOrder. Invalid trace ⇒ Err.

## aoa-budget
- `resolve_closure(root: &Path) -> Result<Closure, BudgetError>`.
- `count_budget(&Closure, target: &str, &Config) -> Result<BudgetReport, BudgetError>`.
- `Config::blocking(ceiling)`, `Config::warn_first(ceiling)`, field `changed_files: Option<BTreeSet<PathBuf>>`.
- `BudgetReport`: `gating_target_tokens`, `verdict`, `files: Vec<FileBudget>` each with `path`, `gating`, `suppression: Option<String>`. `suppressions() -> Vec<(PathBuf,String)>`.
- Suppress marker: `# aoa-allow: oversized-context <reason>` (const `SUPPRESS_MARKER`).
- Tokenizer names accepted: `o200k_base`, `cl100k_base` (+ aliases). Unknown ⇒ Err. **`o200k_base` loads offline** (use as CLI default).

## aoa-lint
- `lint_context(root: &Path, target_tokenizer: &str) -> Result<LintReport, LintError>`.
- `LintReport { budget: BudgetReport, findings: Vec<Finding> }` (derives Serialize).
- `Finding { file: PathBuf, message: String, category: SmellCategory }`.
- Lints the WHOLE closure. `--changed` filtering done CLI-side by intersecting finding paths with the changed set. Suppression honored = file shows in `budget.files` with `suppression=Some` / `gating=false`.
- Verbosity detector fires when a section body > 40 non-blank lines.

## aoa-gap
- `compare(baseline: &RunResult, migrated: &RunResult) -> Result<CompareOutcome, GapError>`.
- `CompareOutcome { gap_delta: f64, held_out_delta: f64, label: Label }` (Serialize).
- `RunResult { tasks: Vec<TaskOutcome>, held_out_provenance, canaries }`. `TaskOutcome { visible_success, held_out_success }`.
- `held_out_provenance` must be `native_composed` or `external` for an available gap (snake_case wire).

## aoa-audit
- `audit(repo: &Path, &AuditConfig) -> Result<AuditReport, AuditError>` — note **AuditConfig**, not AuditOptions. `AuditConfig::default()` works.
- `AuditReport { items: Vec<PunchItem> }` (Serialize). `render_human() -> String`, `has_tier1_gap() -> bool`.
- `exit_code(&AuditReport, fail_on_tier1: bool) -> i32` — non-zero (2) only when fail_on_tier1 AND a Tier-1 gap exists.
- `observe(repo: &Path) -> Result<ObserveOutcome, _>` — only creates `.aoa/` (with `.aoa/.gitignore` = `*`). No tracked-file writes.
- Planes: a bare repo has RuntimeHook+PreCommit+Ci all missing. RuntimeHook & Ci ⇒ Tier-1; PreCommit ⇒ Tier-2.
  - Present RuntimeHook via `.claude/settings.json` OR `.aoa/hooks.toml`; present Ci via `.github/workflows`.
  - **Test for exit-0 under --fail-on tier1**: create `.claude/settings.json` + `.github/workflows/` ⇒ only PreCommit (Tier-2) missing ⇒ no Tier-1 ⇒ exit 0.
  - **Test for exit non-zero under --fail-on tier1**: bare repo ⇒ Tier-1 present ⇒ exit 2.

## aoa-falsify
- `falsify(&FalsifyInput) -> Result<Falsification, FalsifyError>`.
- `FalsifyInput { repos: Vec<PairedRepo>, repeated_run_verdicts: Vec<Verdict>, conventions: Vec<ScoringConvention>, power: PowerAnalysis }` (Deserialize).
- `Falsification { repo_delta_agg, harness_delta_agg, per_repo, verdict, reason }` (Serialize) — written to `falsification.json`; `verdict` field present.
- A minimal well-formed fixture with under-powered/under-quorum input yields `Verdict::Inconclusive` (no error) — perfect for a deterministic test that still produces a `verdict` field.
- `PairedRepo` requires both arms non-empty with ≥1 shared task id, else `MissingArm` error. Use 1 repo with matching task ids ⇒ under quorum ⇒ inconclusive verdict, file still written.

## R-silent forge (criterion 8)
No forge module exists in any crate. CLI implements `src/forge.rs` with `compile_enforcement(forge: &str) -> Result<String, ForgeError>` supporting a small known set (e.g. `github-actions`, `gitlab-ci`) and returning Err for anything else. Wired via `aoa policy compile --forge <name>`. Unknown ⇒ non-zero exit + clear stderr, never a silent no-op.

## R17 dual register
Every audit/eval subcommand takes `--json`. `--json` ⇒ `serde_json` of the library report; default ⇒ human text (`render_human()` / formatted counts). Both renderings exist and are tested.
