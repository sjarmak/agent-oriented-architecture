//! The one seam the readiness commands compose through.
//!
//! `audit`, `recommend`, and `report` all need some prefix of the same chain:
//! measure the repo's audit config, audit it, condition the R9c determination on
//! the signal that audit just measured, and join the migration registry into
//! per-finding recommendations. Each of them used to spell that chain out by
//! hand, and `report` drifted from `recommend` twice (aoa-d6t.35 dropped the
//! signal conditioning, aoa-d6t.41 rebuilt the config as `AuditConfig::default()`
//! and silently lost the measured mutation-surface item). Both shipped: the
//! command exits 0 and prints a plausible readiness view that under-reports.
//!
//! So the chain lives here once, and the commands destructure it. The rule of
//! three would argue against extracting at two callers; the tiebreaker here is
//! defect history, not caller count.
//!
//! This is the composition root's job, not `aoa-audit`'s: that crate has no
//! `aoa-scip-graph` dependency and deliberately never acquires a graph — it
//! scores one the caller supplies.

use std::path::Path;

use anyhow::{Context, Result};

use aoa_audit::AuditReport;
use aoa_gap::ConstructValidityReport;
use aoa_migrate::CodeFix;
use aoa_recommend::RecommendationReport;

/// The audit configuration measured from the repo itself: the defaults plus a
/// best-effort symbol graph indexed from the repo's source (aoa-d6t.23). A
/// repo the indexer cannot see into yields an empty graph, and the audit then
/// withholds the mutation-surface item rather than scoring an empty default —
/// the punch-list must never carry a cost that was not measured.
///
/// An index failure is FATAL here, deliberately and uniformly for all three
/// commands. `aoa_scip_graph::build_symbol_graph` offers the other policy —
/// degrade to an empty graph, never abort — which is right for the R0 scoring
/// path it was built for, where one broken repo should lose its vote rather
/// than sink the run. These commands instead answer a go/no-go question about a
/// single repo, and an empty graph makes the audit withhold the
/// mutation-surface item, so degrading would print a clean punch-list for a
/// repo nobody managed to read.
///
/// The indexer is stricter about source than the audit's own structural scan
/// (aoa-kdqk); reconciling that belongs there, not in a per-caller policy.
fn repo_audit_config(repo: &Path) -> Result<aoa_audit::AuditConfig> {
    let indexed = aoa_scip_graph::index_best_effort(repo)
        .with_context(|| format!("failed to index {}", repo.display()))?;
    Ok(aoa_audit::AuditConfig {
        graph: indexed.graph,
        gold_set: indexed.gold_set,
        ..aoa_audit::AuditConfig::default()
    })
}

/// Audit `repo` through its own measured config — the prefix of the chain that
/// `aoa audit` needs on its own.
pub(crate) fn audited(repo: &Path) -> Result<AuditReport> {
    let cfg = repo_audit_config(repo)?;
    aoa_audit::audit(repo, &cfg).with_context(|| format!("failed to audit {}", repo.display()))
}

/// The full chain, as `recommend` and `report` both need it.
///
/// Derives nothing, unlike its neighbours: `CodeFix` has no `Debug` supertrait,
/// so `#[derive(Debug)]` on `fixes` does not compile.
pub(crate) struct Readiness {
    pub(crate) audit: AuditReport,
    pub(crate) determination: ConstructValidityReport,
    /// The exact registry `recommendations` was joined against. Carried rather
    /// than re-fetched so a caller rendering the migration list cannot describe
    /// a different registry than the one the join consumed.
    pub(crate) fixes: Vec<Box<dyn CodeFix>>,
    pub(crate) recommendations: RecommendationReport,
}

/// Audit `repo`, condition the determination on the behavioral signal the audit
/// measured, and join the migration registry into per-finding recommendations.
///
/// The conditioning is the load-bearing part: a repo with no observe-captured
/// held-out signal must report its behavioral metrics as `InsufficientData`,
/// never `Advisory`. An unconditioned determination would contradict the audit
/// it ships alongside.
pub(crate) fn readiness(repo: &Path) -> Result<Readiness> {
    let audit = audited(repo)?;
    let determination = aoa_gap::determination_with_signal(&audit.behavioral_signal);
    let fixes = aoa_migrate::all_fixes();
    let recommendations = aoa_recommend::recommend(&audit, &determination, &fixes);
    Ok(Readiness {
        audit,
        determination,
        fixes,
        recommendations,
    })
}
