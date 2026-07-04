use anyhow::{Context, Result};
use serde::Serialize;

use crate::cli::ObserveArgs;
use crate::commands::enforce::install_enforce_hooks;
use crate::output::{print_human, print_json};

/// The observe result: what was installed where, identical in both registers
/// (R17). `enforce_settings` is present only when `--enforce` installed the
/// runtime gate.
#[derive(Debug, Serialize)]
struct ObserveView {
    traces_dir: String,
    gitignore: String,
    enforce_settings: Option<String>,
}

/// Install read-only trace telemetry. Touches only the ignored `.aoa/` tree —
/// unless `--enforce` is given, which additionally installs the runtime
/// reproduction-before-mutation gate (R7) into `.claude/settings.json`.
pub fn run(args: &ObserveArgs) -> Result<i32> {
    let outcome = aoa_audit::observe(&args.repo)
        .with_context(|| format!("failed to install telemetry under {}", args.repo.display()))?;

    let enforce_settings = if args.enforce {
        Some(install_enforce_hooks(&args.repo)?.display().to_string())
    } else {
        None
    };

    let view = ObserveView {
        traces_dir: outcome.traces_dir.display().to_string(),
        gitignore: outcome.gitignore.display().to_string(),
        enforce_settings,
    };

    if args.json {
        print_json(&view)?;
    } else {
        let mut message = format!(
            "installed trace telemetry\n  traces dir: {}\n  ignore guard: {}\n",
            view.traces_dir, view.gitignore,
        );
        if let Some(settings) = &view.enforce_settings {
            message.push_str(&format!(
                "  enforcement gate (R7): merged hooks into {settings}\n"
            ));
        }
        print_human(&message);
    }
    Ok(0)
}
