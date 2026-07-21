use anyhow::Result;

use crate::cli::AuditArgs;
use crate::commands::{pipeline, self_audit};
use crate::output::{print_human, print_json};

/// Run a read-only audit and render its tiered punch-list in the requested
/// register. The exit code is driven by `--fail-on tier1`. With `--self` the
/// audit turns on the toolkit itself instead (R14 lint-thyself).
pub fn run(args: &AuditArgs) -> Result<i32> {
    // Before the audit, not after: `--self` measures the toolkit's own manifest
    // and never indexes a repo, so it must not start paying that cost — or
    // start failing on a repo the indexer cannot see into.
    if args.self_audit {
        return self_audit::run(args);
    }

    let report = pipeline::audited(&args.repo)?;

    if args.json {
        print_json(&report)?;
    } else {
        print_human(&report.render_human());
    }

    let fail_on_tier1 = args.fail_on.as_deref() == Some("tier1");
    Ok(aoa_audit::exit_code(&report, fail_on_tier1))
}
