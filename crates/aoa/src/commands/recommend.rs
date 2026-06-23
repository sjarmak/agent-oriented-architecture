use anyhow::{Context, Result};

use crate::cli::RecommendArgs;
use crate::output::{print_human, print_json};

/// Join the audit punch-list, the R9c construct-validity determination, and the
/// migration registry into per-finding recommendations, rendered in the requested
/// register (R17). This is the operator-facing `recommend` pillar: it shows, for
/// each measured finding, whether a fix exists, whether the finding's metric may
/// gate a decision, and the resulting actionable-now vs advisory-only tag.
///
/// Exit code is always 0: surfacing advisory findings must not pressure an
/// operator to "fix" a metric that has not earned gating — that is the Goodhart
/// dynamic the construct-validity determination exists to prevent.
pub fn run(args: &RecommendArgs) -> Result<i32> {
    let cfg = aoa_audit::AuditConfig::default();
    let audit = aoa_audit::audit(&args.repo, &cfg)
        .with_context(|| format!("failed to audit {}", args.repo.display()))?;
    let determination = aoa_gap::current_determination();
    let fixes = aoa_migrate::all_fixes();

    let report = aoa_recommend::recommend(&audit, &determination, &fixes);

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&report.render_human());
    }

    Ok(0)
}
