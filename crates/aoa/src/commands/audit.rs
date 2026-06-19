use anyhow::{Context, Result};

use crate::cli::AuditArgs;
use crate::output::{print_human, print_json};

/// Run a read-only audit and render its tiered punch-list in the requested
/// register. The exit code is driven by `--fail-on tier1`.
pub fn run(args: &AuditArgs) -> Result<i32> {
    let cfg = aoa_audit::AuditConfig::default();
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
