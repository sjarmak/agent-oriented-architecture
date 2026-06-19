//! Context-file linting for the AOA Toolkit.
//!
//! Lints context files (AGENTS.md / CLAUDE.md / rules / READMEs) for
//! agents-lint / cclint-style smells using mechanical, deterministic structural
//! detectors, mapping each finding to a config-smell catalog category from the
//! arXiv:2606.15828 taxonomy ([`SmellCategory`]).
//!
//! The detectors are structural only (per ZFC: no LLM, no semantic judgment) —
//! duplicate headings, oversized sections, dead links, over-broad globs, and
//! structurally contradictory directives.
//!
//! [`lint_context`] reuses the [`aoa_budget`] closure to know which files to
//! lint and COMPOSES the budget result (resolved file set + token budget) with
//! the smell findings in a single [`LintReport`].

mod category;
mod detectors;
mod error;
mod finding;
mod lint;
mod report;

pub use category::SmellCategory;
pub use error::LintError;
pub use finding::Finding;
pub use lint::lint_context;
pub use report::LintReport;
