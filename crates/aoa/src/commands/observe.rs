use anyhow::{Context, Result};

use crate::cli::ObserveArgs;
use crate::output::print_human;

/// Install read-only trace telemetry. Touches only the ignored `.aoa/` tree.
pub fn run(args: &ObserveArgs) -> Result<i32> {
    let outcome = aoa_audit::observe(&args.repo)
        .with_context(|| format!("failed to install telemetry under {}", args.repo.display()))?;

    print_human(&format!(
        "installed trace telemetry\n  traces dir: {}\n  ignore guard: {}\n",
        outcome.traces_dir.display(),
        outcome.gitignore.display(),
    ));
    Ok(0)
}
