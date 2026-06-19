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
    #[command(subcommand)]
    pub command: EvalCommand,
}

#[derive(Debug, Subcommand)]
pub enum EvalCommand {
    /// Validate a trace file: print per-type span counts; exit non-zero if invalid.
    ValidateTrace(ValidateTraceArgs),

    /// Compare a baseline run against a migrated run; print the gap delta.
    Compare(CompareArgs),

    /// Post-process a codeprobe run into per-task AOA metric records.
    Run(EvalRunArgs),

    /// R0b held-out integrity: compose the AOA leakage canary over two
    /// dual-verifier codeprobe runs (baseline vs migrated).
    R0b(R0bArgs),
}

#[derive(Debug, Args)]
pub struct ValidateTraceArgs {
    /// Trace JSON file to validate.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct CompareArgs {
    /// Baseline run-result JSON.
    #[arg(value_name = "BASELINE")]
    pub baseline: PathBuf,

    /// Migrated run-result JSON.
    #[arg(value_name = "MIGRATED")]
    pub migrated: PathBuf,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct EvalRunArgs {
    /// codeprobe run directory: the config-label dir holding `<task_id>/`
    /// subtrees, each with `agent_output.txt` and `scoring.json`.
    #[arg(long, value_name = "DIR")]
    pub codeprobe_run: PathBuf,

    /// codeprobe task-source dir (one `<task_id>/` per task) supplying the
    /// oracle/gold set via the bench loader. Without it, gold-set-dependent
    /// metrics report no anchors and the gap may be unavailable.
    #[arg(long, value_name = "DIR")]
    pub tasks: Option<PathBuf>,

    /// Vendored SCIP index for the task repo (high-confidence graph). Mutually
    /// exclusive with `--repo`.
    #[arg(long, value_name = "FILE", conflicts_with = "repo")]
    pub scip_index: Option<PathBuf>,

    /// Task repo checkout for a best-effort (low-confidence) graph. Without a
    /// graph source the symbol graph degrades to zero weight (R0-ineligible).
    #[arg(long, value_name = "DIR")]
    pub repo: Option<PathBuf>,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct R0bArgs {
    /// Baseline codeprobe run directory (dual-verifier `dual_composite` scoring):
    /// the config-label dir holding `<task_id>/scoring.json` subtrees.
    #[arg(long, value_name = "DIR")]
    pub baseline: PathBuf,

    /// Migrated codeprobe run directory, same layout as `--baseline`.
    #[arg(long, value_name = "DIR")]
    pub migrated: PathBuf,

    /// codeprobe task-source dir (one `<task_id>/` per task) supplying each
    /// task's oracle via the bench loader. Used to classify held-out provenance;
    /// a run whose tasks have no independent held-out leg yields gap:unavailable.
    #[arg(long, value_name = "DIR")]
    pub tasks: PathBuf,

    /// Canary manifest JSON: `[{"id": "<task_id>", "expected_held_out": <bool>}]`.
    /// Each entry is a known held-out probe whose clean expectation is declared by
    /// the operator; an observed held-out outcome that diverges is a leakage flip.
    #[arg(long, value_name = "FILE")]
    pub canary: Option<PathBuf>,

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
