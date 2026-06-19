use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// The default target tokenizer for context-budget probes. `o200k_base` loads
/// without network access, so it is the safe CLI default.
pub const DEFAULT_TOKENIZER: &str = "o200k_base";

/// The AOA Toolkit command-line interface.
#[derive(Debug, Parser)]
#[command(name = "aoa", version, about = "AOA Toolkit CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install read-only trace telemetry under the ignored `.aoa/` tree.
    Observe(ObserveArgs),

    /// Print a tiered, ranked audit punch-list grounded in measured numbers.
    Audit(AuditArgs),

    /// Lint context files for config-file smells over the resolved closure.
    LintContext(LintArgs),

    /// Evaluation gates: trace validation and the reward-hacking gap compare.
    Eval(EvalArgs),

    /// Run the wrong-layer falsification gate and write `falsification.json`.
    Falsify(FalsifyArgs),

    /// Enforcement-plane policy utilities (fail-loud forge adapters).
    Policy(PolicyArgs),
}

#[derive(Debug, Args)]
pub struct ObserveArgs {
    /// Repository root to install telemetry into. Defaults to the cwd.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

#[derive(Debug, Args)]
pub struct AuditArgs {
    /// Repository root to audit. Defaults to the cwd.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Exit non-zero when a Tier-1 gap is present.
    #[arg(long, value_parser = ["tier1"])]
    pub fail_on: Option<String>,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct LintArgs {
    /// Root context document to resolve the closure from.
    #[arg(long, default_value = "AGENTS.md")]
    pub root: PathBuf,

    /// Restrict the reported findings to this set of changed files.
    #[arg(long, num_args = 1.., value_delimiter = ' ')]
    pub changed: Vec<PathBuf>,

    /// Target tokenizer for the composed budget report.
    #[arg(long, default_value = DEFAULT_TOKENIZER)]
    pub tokenizer: String,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct EvalArgs {
    /// Validate a trace file: print per-type span counts; exit non-zero if invalid.
    #[arg(long, value_name = "FILE")]
    pub validate_trace: Option<PathBuf>,

    /// Compare a baseline run against a migrated run; print the gap delta.
    #[arg(long, num_args = 2, value_names = ["BASELINE", "MIGRATED"])]
    pub compare: Option<Vec<PathBuf>>,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct FalsifyArgs {
    /// Falsification input JSON (paired-repo evidence, power, conventions).
    #[arg(long, value_name = "INPUT")]
    pub repos: PathBuf,

    /// Where to write `falsification.json`. Defaults to the cwd.
    #[arg(long, default_value = "falsification.json")]
    pub out: PathBuf,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// Compile the enforcement plane for a forge. Fails loudly on an unknown forge.
    Compile {
        /// The forge to compile enforcement for (e.g. `github-actions`).
        #[arg(long)]
        forge: String,
    },
}
