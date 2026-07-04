use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::AuditArgs;
use crate::commands::self_audit;
use crate::output::{print_human, print_json};

/// The audit configuration measured from the repo itself: the defaults plus a
/// best-effort symbol graph indexed from the repo's source (aoa-d6t.23). A
/// repo the indexer cannot see into yields an empty graph, and the audit then
/// withholds the mutation-surface item rather than scoring an empty default —
/// the punch-list must never carry a cost that was not measured.
pub(crate) fn repo_audit_config(repo: &Path) -> Result<aoa_audit::AuditConfig> {
    let indexed = aoa_scip_graph::index_best_effort(repo)
        .with_context(|| format!("failed to index {}", repo.display()))?;
    Ok(aoa_audit::AuditConfig {
        graph: indexed.graph,
        gold_set: indexed.gold_set,
        ..aoa_audit::AuditConfig::default()
    })
}

/// Run a read-only audit and render its tiered punch-list in the requested
/// register. The exit code is driven by `--fail-on tier1`. With `--self` the
/// audit turns on the toolkit itself instead (R14 lint-thyself).
pub fn run(args: &AuditArgs) -> Result<i32> {
    if args.self_audit {
        return self_audit::run(args);
    }

    let cfg = repo_audit_config(&args.repo)?;
    let report = aoa_audit::audit(&args.repo, &cfg)
        .with_context(|| format!("failed to audit {}", args.repo.display()))?;

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&report.render_human());
    }

    let fail_on_tier1 = args.fail_on.as_deref() == Some("tier1");
    Ok(aoa_audit::exit_code(&report, fail_on_tier1))
}
