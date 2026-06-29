use anyhow::{Context, Result};

use crate::cli::ObserveArgs;
use crate::commands::enforce::install_enforce_hooks;
use crate::output::print_human;

/// Install read-only trace telemetry. Touches only the ignored `.aoa/` tree —
/// unless `--enforce` is given, which additionally installs the runtime
/// reproduction-before-mutation gate (R7) into `.claude/settings.json`.
pub fn run(args: &ObserveArgs) -> Result<i32> {
    let outcome = aoa_audit::observe(&args.repo)
        .with_context(|| format!("failed to install telemetry under {}", args.repo.display()))?;

    let mut message = format!(
        "installed trace telemetry\n  traces dir: {}\n  ignore guard: {}\n",
        outcome.traces_dir.display(),
        outcome.gitignore.display(),
    );

    if args.enforce {
        let settings = install_enforce_hooks(&args.repo)?;
        message.push_str(&format!(
            "  enforcement gate (R7): merged hooks into {}\n",
            settings.display(),
        ));
    }

    print_human(&message);
    Ok(0)
}
