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

    /// Apply safe, reproducible, code-layer migrations toward the structure
    /// best-practices the audit measures (the R0 repo-delta treatment).
    Migrate(MigrateArgs),

    /// Lint context files for config-file smells over the resolved closure.
    LintContext(LintArgs),

    /// Evaluation gates: trace validation and the reward-hacking gap compare.
    Eval(EvalArgs),

    /// Surface the R9c construct-validity determination: which gating-candidate
    /// metrics may gate a decision (Gating) and which are advisory-only.
    Gap(GapArgs),

    /// Join audit findings, construct-validity mode, and migration availability
    /// into per-finding recommendations (actionable-now vs advisory-only).
    Recommend(RecommendArgs),

    /// Run the wrong-layer falsification gate and write `falsification.json`.
    Falsify(FalsifyArgs),

    /// Enforcement-plane policy utilities (fail-loud forge adapters).
    Policy(PolicyArgs),

    /// Runtime enforcement hook entry points (R7). Invoked by the Claude Code
    /// hooks that `aoa observe --enforce` installs; reads the payload on stdin.
    Enforce(EnforceArgs),
}

#[derive(Debug, Args)]
pub struct ObserveArgs {
    /// Repository root to install telemetry into. Defaults to the cwd.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Also install the runtime reproduction-before-mutation gate (R7) by
    /// merging the enforcement hooks into `.claude/settings.json`. Without this
    /// flag `observe` stays zero-write toward tracked files.
    #[arg(long)]
    pub enforce: bool,
}

#[derive(Debug, Args)]
pub struct EnforceArgs {
    #[command(subcommand)]
    pub command: EnforceCommand,
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum EnforceCommand {
    /// PostToolUse: append a `test.run` span when a Bash command runs tests.
    Record,

    /// PreToolUse: block a pending write unless reproduction precedes it.
    Check,
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
pub struct MigrateArgs {
    /// Repository checkout to migrate. Defaults to the cwd.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Write the changes (archived + recorded for rollback). Without it the
    /// command is a dry-run that only previews the diff (safe by default).
    #[arg(long)]
    pub apply: bool,

    /// Undo the last applied migration recorded in `.aoa/migrate/manifest.json`.
    #[arg(long, conflicts_with = "apply")]
    pub rollback: bool,

    /// Restrict the run to these fix ids (repeatable). Empty runs every
    /// registered fix. Lets an R0 campaign pin the exact treatment set.
    /// Ignored by `--rollback` (which reverts the whole recorded manifest), so
    /// the two are mutually exclusive.
    #[arg(long = "fix", value_name = "ID", conflicts_with = "rollback")]
    pub fix: Vec<String>,

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

    /// Build an R0 falsification input from a codeprobe experiment's paired
    /// config arms (repo-arm vs harness-arm over the same mined tasks). Emits the
    /// `FalsifyInput` JSON `aoa falsify` consumes, plus a build report.
    Experiment(ExperimentArgs),
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
pub struct ExperimentArgs {
    /// Build manifest JSON: per-repo confidence + calibration (operator-declared)
    /// and, per fixed-seed run, the paths to the repo-arm and harness-arm
    /// codeprobe config-label run dirs. See `docs/r0_runbook.md`.
    #[arg(long, value_name = "FILE")]
    pub manifest: PathBuf,

    /// codeprobe task-source dir (one `<task_id>/` per task), shared across arms,
    /// supplying each task's oracle for held-out provenance classification.
    #[arg(long, value_name = "DIR")]
    pub tasks: PathBuf,

    /// Where to write the `FalsifyInput` JSON `aoa falsify --repos` consumes.
    #[arg(long, value_name = "FILE", default_value = "falsify_input.json")]
    pub out: PathBuf,

    /// Emit the structured JSON build report instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct GapArgs {
    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct RecommendArgs {
    /// Repository root to audit and recommend over. Defaults to the cwd.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Emit the structured JSON rendering instead of human text.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct FalsifyArgs {
    /// Falsification input JSON (paired-repo evidence, power, conventions).
    #[arg(long, value_name = "INPUT")]
    pub repos: PathBuf,

    /// Build report JSON emitted alongside the `FalsifyInput` by
    /// `aoa eval experiment`. When it flags degraded convention inputs, the gate
    /// abstains (verdict `inconclusive`, `precondition_unmet`) rather than
    /// asserting a verdict the R0' convention-invariance check cannot back.
    #[arg(long, value_name = "FILE")]
    pub build_meta: Option<PathBuf>,

    /// codeprobe `experiment aggregate` output (`reports/aggregate.json`). Its
    /// `bias_warnings` are surfaced ALONGSIDE the AOA verdict, never mutating it;
    /// a `no_independent_baseline` warning is flagged as gate-invalidating.
    #[arg(long, value_name = "FILE")]
    pub bias_warnings: Option<PathBuf>,

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
    /// Compile `aoa-policy.yaml` into the three enforcement planes (R5): the
    /// runtime hooks, the pre-commit guard, and the CI workflow + CODEOWNERS.
    /// Deterministic and idempotent — a re-run is a no-op diff.
    Compile {
        /// Repository root holding `aoa-policy.yaml`. Defaults to the cwd.
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// The forge to compile the CI plane for. Fails loudly on an unknown forge.
        #[arg(long, default_value = "github-actions")]
        forge: String,
    },

    /// Pre-commit plane entry point: exit non-zero if any of the given staged
    /// files matches a protected path in `aoa-policy.yaml`.
    GuardStaged {
        /// Repository root holding `aoa-policy.yaml`. Defaults to the cwd.
        #[arg(long, default_value = ".")]
        repo: PathBuf,

        /// Staged files to check (supplied by pre-commit).
        #[arg(value_name = "FILE")]
        files: Vec<PathBuf>,
    },
}
