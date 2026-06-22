use anyhow::Result;

use crate::cli::GapArgs;
use crate::output::{print_human, print_json};

/// Surface the R9c construct-validity determination in the requested register.
///
/// This is the live, operator-facing consumer of
/// [`aoa_gap::current_determination`]: it reports which gating-candidate metrics
/// have earned `Gating` status (a confirming external-outcome correlation) and
/// which remain `Advisory`. With no external-outcome corpus available, every
/// candidate is advisory — the surface shows that explicitly rather than letting
/// any metric silently gate a decision.
pub fn run(args: &GapArgs) -> Result<i32> {
    let report = aoa_gap::current_determination();

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&report.render_human());
    }

    Ok(0)
}
