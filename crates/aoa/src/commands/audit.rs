use anyhow::Result;
use serde::Serialize;

use crate::cli::AuditArgs;
use crate::commands::enforce::enforce_hook_warning;
use crate::commands::{pipeline, self_audit};
use crate::output::{print_human, print_json};

#[derive(Serialize)]
struct AuditView<'a> {
    #[serde(flatten)]
    report: &'a aoa_audit::AuditReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    enforce_hook_warning: Option<&'a str>,
}

/// Run a read-only audit and render its tiered punch-list in the requested
/// register. The exit code is driven by `--fail-on tier1`. With `--self` the
/// audit turns on the toolkit itself instead (R14 lint-thyself).
pub fn run(args: &AuditArgs) -> Result<i32> {
    if args.self_audit {
        return self_audit::run(args);
    }

    let report = pipeline::audited(&args.repo)?;
    let hook_warning = enforce_hook_warning(&args.repo);

    if args.json {
        print_json(&AuditView {
            report: &report,
            enforce_hook_warning: hook_warning.as_deref(),
        })?;
    } else {
        let mut rendered = report.render_human();
        if let Some(warning) = &hook_warning {
            rendered.push_str(&format!("warning: {warning}\n"));
        }
        print_human(&rendered);
    }

    let fail_on_tier1 = args.fail_on.as_deref() == Some("tier1");
    Ok(aoa_audit::exit_code(&report, fail_on_tier1))
}
