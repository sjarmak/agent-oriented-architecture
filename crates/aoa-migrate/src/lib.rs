//! Safe, reproducible, oracle-blind, code-layer repo migrations — the R0
//! repo-delta treatment AOA claims as its layer.
//!
//! The flow mirrors `aoa-budget`'s archive-then-write fix, generalized to a
//! plan/apply engine:
//!
//! - A [`CodeFix`] *plans* changes by reading the checkout (writing nothing),
//!   returning [`PlannedChange`]s.
//! - [`MigrationPlan::build`] aggregates fixes into one plan whose
//!   [`render_diff`](MigrationPlan::render_diff) is the `--plan` preview.
//! - [`apply`] executes a plan reversibly, recording a [`MigrationManifest`]
//!   under the ignored `.aoa/migrate/` tree; [`rollback`] undoes it.
//!
//! Two fixes ship today. [`NavigabilityAnchorFix`] consumes the audit's
//! navigability sites and writes a README whose content is a pure function of
//! the directory tree — mechanical, deterministic, and blind to file bodies, so
//! it cannot leak a held-out task answer. [`DeadImportFix`] removes
//! rustc-certified unused imports via an isolated `cargo check` + `rustfix`,
//! producing strictly-subtractive `Overwrite` changes. Both fixes' construct-
//! validity guardrails are documented on the fix and in `docs/r0_runbook.md`.

mod apply;
mod error;
mod fix;
mod imports;
mod plan;

pub use apply::{apply, manifest_path, read_manifest, rollback, ManifestEntry, MigrationManifest};
pub use error::MigrateError;
pub use fix::{
    all_fixes, ChangeAction, CodeFix, FixEligibility, FixProvenance, NavigabilityAnchorFix,
    PlannedChange,
};
pub use imports::DeadImportFix;
pub use plan::MigrationPlan;
