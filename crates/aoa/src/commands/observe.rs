use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::cli::ObserveArgs;
use crate::commands::enforce::merge_enforce_hooks;
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

/// Merge the enforcement hooks into `<repo>/.claude/settings.json`, creating the
/// file and its parent if absent. Idempotent: an existing file is parsed, merged,
/// and rewritten, so a re-run that changes nothing leaves the file byte-stable.
fn install_enforce_hooks(repo: &Path) -> Result<PathBuf> {
    let settings_path = repo.join(".claude").join("settings.json");

    let existing = match std::fs::read_to_string(&settings_path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .with_context(|| format!("{} is not valid JSON", settings_path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", settings_path.display()))
        }
    };

    let merged = merge_enforce_hooks(existing);

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered =
        serde_json::to_string_pretty(&merged).context("failed to render settings.json")?;
    std::fs::write(&settings_path, format!("{rendered}\n"))
        .with_context(|| format!("failed to write {}", settings_path.display()))?;

    Ok(settings_path)
}
