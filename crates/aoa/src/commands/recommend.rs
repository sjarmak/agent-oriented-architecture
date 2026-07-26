use anyhow::Result;

use crate::cli::RecommendArgs;
use crate::commands::pipeline::{self, Readiness};
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
///
/// The determination is conditioned on the repo's behavioral signal (counted by
/// the audit from the observe-captured corpus, aoa-d6t.23): a repo with no
/// observe-captured held-out signal reports its behavioral metrics as
/// InsufficientData, never Advisory.
pub fn run(args: &RecommendArgs) -> Result<i32> {
    let Readiness {
        audit,
        recommendations: report,
        ..
    } = pipeline::readiness(&args.repo)?;
    // The recommendation report carries no warning field of its own; a degraded
    // subtree discovery in the underlying audit surfaces here. It stays in the
    // command rather than in the seam: `report` already carries the same warning
    // on the wire, so warning from `readiness` would give `aoa report` a stderr
    // line it has never printed.
    if let Some(warning) = &audit.subtree_discovery_warning {
        eprintln!("warning: {warning}");
    }
    if args.json {
        print_json(&report)?;
    } else {
        print_human(&report.render_human());
    }

    Ok(0)
}
