//! Context budget gate for the AOA Toolkit.
//!
//! Resolves the transitive closure of context files reachable from a root
//! document (markdown links and `@path` includes), counts tokens under a pinned
//! reference encoding ([`o200k_base`](tokenizer::REFERENCE_ENCODING)) AND a
//! configurable target-model tokenizer, and enforces a declared ceiling. The
//! gate is GATED on the target tokenizer while reporting both totals.
//!
//! Inline `# aoa-allow: oversized-context <reason>` markers suppress a file's
//! contribution to the gate (the reason is captured in the report), diff-scoped
//! evaluation restricts the gating set to a provided changed-file list, and a
//! [`fix_oversized`] operation mechanically splits an over-budget file into
//! linked sub-files and re-checks to green.
//!
//! Downstream crates (aoa-lint, aoa-audit) drive this through
//! [`resolve_closure`] + [`count_budget`].

mod budget;
mod closure;
mod error;
mod fix;
mod reference;
mod suppress;
mod tokenizer;

pub use budget::{count_budget, BudgetReport, Config, FileBudget, Verdict};
pub use closure::{resolve_closure, Closure, ContextFile};
pub use error::BudgetError;
pub use fix::{fix_oversized, FixOutcome};
pub use reference::{extract_references, Reference};
pub use suppress::{find_suppression, SUPPRESS_MARKER};
pub use tokenizer::{count_tokens, reference_encoder, target_encoder, REFERENCE_ENCODING};
