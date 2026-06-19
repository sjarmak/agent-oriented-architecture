# Plan — unit-context-lint (R13)

## Crate layout (crates/aoa-lint)

```
Cargo.toml                      # aoa-budget path dep, serde, serde_json, thiserror
src/lib.rs                      # public API re-exports + module wiring
src/category.rs                 # SmellCategory enum + stable ids (taxonomy mapping)
src/finding.rs                  # Finding struct
src/report.rs                   # LintReport { budget, findings }
src/error.rs                    # LintError (thiserror, wraps BudgetError)
src/lint.rs                     # lint_context(): resolve closure, count budget, run detectors
src/detectors/mod.rs            # detector registry (Vec of fn over a ContextFile-like view)
src/detectors/duplication.rs    # duplicate headings
src/detectors/verbosity.rs      # oversized section
src/detectors/stale_reference.rs# dead local markdown link
src/detectors/overbroad_glob.rs # bare ** glob
src/detectors/contradiction.rs  # always/never on same token
tests/lint.rs                   # all 4 acceptance criteria
tests/fixtures/tree/AGENTS.md    # triggers contradiction + glob + stale link
tests/fixtures/tree/rules/README.md  # triggers duplication + verbosity
```

## Public API

- `pub fn lint_context(root: &Path, target_tokenizer: &str) -> Result<LintReport, LintError>`
- `LintReport { pub budget: BudgetReport, pub findings: Vec<Finding> }` (Serialize/Deserialize)
- `Finding { pub file: PathBuf, pub message: String, pub category: SmellCategory }`
- `SmellCategory` enum with `pub fn id(&self) -> &'static str`

## Detector contract

Each detector: `fn(file: &LintedFile) -> Vec<Finding>` where `LintedFile { path, text }` derived
from closure's `ContextFile`. Detectors are deterministic, mechanical, no LLM, no IO except the
stale-reference detector which checks `Path::exists` on resolved link targets (filesystem
structural check, allowed).

## Flow (ZFC-respecting)

1. `resolve_closure(root)` (IO) -> closure.
2. `count_budget(&closure, target, &Config::blocking(LINT_BUDGET_CEILING))` -> BudgetReport (reuse).
3. For each closure file, run all detectors, collect findings (mechanical).
4. Return `LintReport { budget, findings }`.

## Decisions

- count_budget needs a Config (3 args) — use a large default ceiling so the budget section is
  informational, not a gate (lint composes results; it does not re-gate the budget).
- Stale-reference detector resolves link targets relative to the file's parent dir, mirroring
  aoa-budget's reference resolution, and flags non-existent local targets only.
