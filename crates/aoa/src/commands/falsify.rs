use anyhow::{Context, Result};

use aoa_falsify::FalsifyInput;

use crate::cli::FalsifyArgs;
use crate::output::{print_human, print_json};

/// Run the R0 falsification gate over the paired-repo input and write the
/// verdict-bearing `falsification.json`.
pub fn run(args: &FalsifyArgs) -> Result<i32> {
    let raw = std::fs::read_to_string(&args.repos)
        .with_context(|| format!("failed to read falsify input {}", args.repos.display()))?;
    let input: FalsifyInput = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse falsify input {}", args.repos.display()))?;

    let result = aoa_falsify::falsify(&input).context("falsification gate failed")?;

    let serialized = serde_json::to_string_pretty(&result)?;
    std::fs::write(&args.out, &serialized)
        .with_context(|| format!("failed to write {}", args.out.display()))?;

    if args.json {
        print_json(&result)?;
    } else {
        print_human(&format!(
            "falsification verdict: {:?}\n  repo-delta: {:.4}\n  harness-delta: {:.4}\n  notes: {}\n  written: {}\n",
            result.verdict,
            result.repo_delta,
            result.harness_delta,
            result.notes.join("; "),
            args.out.display(),
        ));
    }
    Ok(0)
}
