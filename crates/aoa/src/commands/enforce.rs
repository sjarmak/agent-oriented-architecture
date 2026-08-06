//! The runtime plane of the reproduction-before-mutation gate (R7), invoked as
//! Claude Code hooks installed by `aoa observe --enforce`.
//!
//! Two hook entry points, dispatched by [`EnforceCommand`]:
//!
//! - **`record`** (PostToolUse on `Bash`): when a Bash command runs a test
//!   suite, append a `test.run` span to an append-only live log. Recording never
//!   blocks — it always exits 0.
//! - **`check`** (PreToolUse on the mutation tools): consult [`aoa_enforce`]'s
//!   reproduction gate against the live log; if no reproduction precedes the
//!   pending write, append a `write.blocked` span and exit 2 (the Claude Code
//!   signal that blocks the tool call), surfacing the reason on stderr. An
//!   *allowed* write is recorded as a `write.attempt` span carrying its target
//!   path — intent, not outcome. Checking fails **closed**: a check that cannot
//!   run at all still exits 2 rather than waving the write through (see
//!   [`run`]).
//! - **`commit`** / **`fail`** / **`deny`** (PostToolUse, PostToolUseFailure,
//!   and PermissionDenied on the mutation tools): append `write.committed`,
//!   `write.failed`, or `write.denied` respectively.
//!
//! Intent and outcome are deliberately separate records. A `write.attempt` is
//! written before the tool runs and therefore proves nothing about whether the
//! file changed; only `write.committed` does, and it alone feeds the held-out
//! ground truth the live corpus accumulates (aoa-d6t.23). Treating the attempt
//! as the landed edit is what let failed, denied, and abandoned mutations
//! contaminate that corpus.
//!
//! Nothing here classifies a tool response to decide which outcome occurred.
//! The host raises a distinct event per outcome, so the routing is structural:
//! whichever subcommand the host invoked *is* the answer.
//!
//! # Upgrading an existing install
//!
//! The outcome hooks are written into `.claude/settings.json` by
//! [`install_enforce_hooks`], which runs from `aoa observe --enforce` and
//! `aoa policy compile` — nothing re-runs it on upgrade. A repo whose settings
//! predate these hooks keeps recording attempts and never records an outcome,
//! so its sessions supply no held-out edits. That surfaces as an explicit
//! `InsufficientData` reason from `aoa audit` rather than as a confident score
//! over zero evidence, but the fix is to re-run `aoa observe --enforce`.
//!
//! That remedy is only worth documenting because installation now fails loudly
//! when it cannot be applied. A hand-edited `settings.json` — a non-object file,
//! a non-object `hooks`, a non-array event, or one of these commands already
//! registered under a different matcher — is reported with the offending file
//! and key, and the operator's file is left untouched. Each of those shapes was
//! previously swallowed (or, for a non-object `hooks`, a panic), so re-running
//! the documented remedy silently changed nothing and the repo went on reading
//! as greenfield.
//!
//! The live log is owned by AOA rather than read from the host (approach (a)):
//! we control its format, so the gate reads exactly the spans we wrote — no
//! dependency on the host's transcript format. It lands under the same ignored
//! `.aoa/traces/` tree that `observe` already provisions. The store itself lives
//! in [`aoa_enforce::live_log`], beside the gates that decide over it; this
//! module resolves the trust root, dispatches, and renders.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use aoa_codeprobe_shim::bash_runs_tests;
use aoa_enforce::{
    blocked_span, generated_artifact_gate, reproduction_gate, BlockReason, Decision, LiveLog,
    TornTailRepair,
};
use aoa_policy::Policy;
use aoa_trace::{normalize_lexically, resolve_canonicalizing, PathTrustError, SpanType};

use crate::cli::{EnforceArgs, EnforceCommand};
use crate::commands::generated::generated_rules;
use crate::output::eprint_human;

/// The tools whose writes the gate guards. A pending call to any of these is a
/// mutation and must be preceded by a reproduction (`test.run`) span.
const MUTATION_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

const ENFORCE_HOOK_SET_VERSION: u64 = 2;
const AOA_SETTINGS_KEY: &str = "aoa";
const HOOK_VERSION_KEY: &str = "enforce_hook_set_version";

/// The wrapper every installed hook runs, relative to the repository root.
const ENFORCE_WRAPPER_REL: &str = ".claude/hooks/aoa-enforce";

/// The wrapper's contents, held here so the installer and the drift test read
/// one source rather than two copies that can disagree.
const ENFORCE_WRAPPER_SCRIPT: &str = include_str!("enforce_hook.sh");

/// The command Claude Code runs for one enforce `verb`.
///
/// Hooks run under `/bin/sh` with the host's environment, so the earlier bare
/// `aoa enforce <verb>` form enforced only where the binary happened to be on
/// that environment's PATH. Where it was not, the hooks failed non-blocking and
/// the plane was inert while still reading as installed — the exact
/// silently-degraded guard AOA exists to detect. The command now names a
/// repo-local wrapper that resolves the binary itself and is loud when it
/// cannot. `CLAUDE_PROJECT_DIR` is the host's repo root; the `.` fallback keeps
/// the command runnable from the repo root by hand and under a scrubbed
/// environment, which is how the resolution criterion is checked.
fn hook_command(verb: &str) -> String {
    format!("\"${{CLAUDE_PROJECT_DIR:-.}}\"/{ENFORCE_WRAPPER_REL} {verb}")
}

/// The exit code Claude Code reads as "deny this tool call"; every other
/// non-zero exit is only a non-blocking warning.
const BLOCK_EXIT_CODE: i32 = 2;

/// The Claude Code matcher selecting exactly [`MUTATION_TOOLS`]. Derived from
/// that list rather than spelled out again, so adding a guarded tool cannot
/// leave the hooks matching the old set. Every mutation hook AOA installs
/// shares it.
fn mutation_tool_matcher() -> String {
    MUTATION_TOOLS.join("|")
}

/// The subset of a Claude Code hook payload this gate needs. Unknown fields are
/// ignored by serde, so the host may add more without breaking the parse.
#[derive(Debug, Deserialize)]
struct HookEvent {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: Map<String, Value>,
    /// Project directory the host invoked the hook from; the live log is rooted
    /// here. Absent payloads fall back to the process cwd.
    #[serde(default)]
    cwd: String,
}

/// Entry point wired into the CLI. Reads the hook payload from stdin and routes
/// to the record or check path.
///
/// `check` fails **closed**: any error reaching this point means the gate could
/// not evaluate the pending write, and an unevaluated write is denied. Returning
/// the error instead would exit 1, which the host reads as a non-blocking
/// warning — so a log it cannot open (a directory or FIFO squatting the path, an
/// unwritable file, a lock that never frees) would disable R5, R6 and R7 for the
/// whole session while the tool call sailed through.
///
/// Every other subcommand keeps failing open, and that asymmetry is the point:
/// they report history after the host has already settled the outcome, so there
/// is nothing left to deny and blocking on a bookkeeping error would fail a call
/// the gate itself allowed.
///
/// The accepted cost: anything that can durably break the live log — a full
/// disk, a stripped permission, another process holding the lock past the
/// store's bounded wait — now denies every write for the rest of the session instead
/// of degrading quietly. That is a real availability lever, and it is the one we
/// want: it takes write access to `.aoa/traces/` under the agent's own user, so
/// whoever can pull it could already edit the repo directly, and a loud
/// session-wide stop is recoverable by a human where a silently disabled gate is
/// not.
pub fn run(args: &EnforceArgs) -> Result<i32> {
    run_with_failure_posture(args.command, || {
        read_event().and_then(|event| match args.command {
            EnforceCommand::Record => run_record(&event),
            EnforceCommand::Check => run_check(&event),
            EnforceCommand::Commit => run_outcome(&event, SpanType::WriteCommitted),
            EnforceCommand::Fail => run_outcome(&event, SpanType::WriteFailed),
            EnforceCommand::Deny => run_outcome(&event, SpanType::WriteDenied),
        })
    })
}

fn run_with_failure_posture(
    command: EnforceCommand,
    operation: impl FnOnce() -> Result<i32>,
) -> Result<i32> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Err(err)) if denies_on_failure(command) => {
            // The tool name lives in the payload, which may be the thing that
            // failed to parse, so the message names the gate rather than the
            // call it is denying.
            eprint_human(&format!(
                "aoa: blocked — the write gate could not evaluate this call: {err:#}"
            ));
            Ok(BLOCK_EXIT_CODE)
        }
        Ok(outcome) => outcome,
        Err(_) if denies_on_failure(command) => {
            eprint_human("aoa: blocked — the write gate panicked while evaluating this call");
            Ok(BLOCK_EXIT_CODE)
        }
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Whether a failure in this hook must deny the pending tool call.
///
/// Spelled out per variant rather than as `Check` plus a default, so a
/// subcommand added later has to state its posture here. A catch-all would hand
/// every future hook the fail-open answer by omission — the wrong direction for
/// anything that gates a write, and the exact defect this function exists to
/// keep from recurring.
fn denies_on_failure(command: EnforceCommand) -> bool {
    match command {
        EnforceCommand::Check => true,
        EnforceCommand::Record
        | EnforceCommand::Commit
        | EnforceCommand::Fail
        | EnforceCommand::Deny => false,
    }
}

/// Read and parse the hook payload the host writes to stdin.
fn read_event() -> Result<HookEvent> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("failed to read hook payload from stdin")?;
    serde_json::from_str(&raw).context("hook payload was not valid JSON with the expected fields")
}

/// Record the settled outcome of a mutation, one span type per hook event.
///
/// The caller has already decided which outcome this is by virtue of which hook
/// event fired, so this never inspects `tool_response` — a payload whose shape
/// the host does not document and which carries no typed success flag anyway.
/// Recording never blocks: an outcome hook reports history, and failing the
/// tool call after the fact would be both useless and destructive.
///
/// A non-mutation tool records nothing: these hooks are registered per matcher,
/// but a matcher is host configuration and a stale or hand-edited
/// `settings.json` can route anything here.
fn run_outcome(event: &HookEvent, span_type: SpanType) -> Result<i32> {
    if !MUTATION_TOOLS.contains(&event.tool_name.as_str()) {
        return Ok(0);
    }
    if write_target(event).is_some() {
        let base = resolve_base(event)?;
        record_write_span(&base, event, span_type)?;
    }
    Ok(0)
}

/// Append one write-lifecycle span carrying the event's target path.
///
/// A mutation call with no resolvable target records nothing: there is no path
/// to hold out, and a pathless write span would be indistinguishable from one
/// whose target was dropped.
fn record_write_span(base: &Path, event: &HookEvent, span_type: SpanType) -> Result<()> {
    if let Some(target) = write_target(event) {
        let log = LiveLog::for_session(base, &event.session_id);
        let mut attributes = Map::new();
        attributes.insert("path".to_string(), Value::String(target.to_string()));
        report_repair(&log, log.append(span_type, attributes)?);
    }
    Ok(())
}

/// Tell the operator when an append discarded a torn tail left by an earlier
/// crash.
///
/// [`aoa_enforce::live_log`] returns the repair as a fact rather than printing
/// it, because a store has no business choosing an output channel. Deciding that
/// the channel is stderr is this layer's job, and saying nothing would let a
/// truncation that lost real spans pass unremarked.
fn report_repair(log: &LiveLog, repair: Option<TornTailRepair>) {
    if let Some(TornTailRepair { discarded_bytes }) = repair {
        eprint_human(&format!(
            "aoa: repaired {} by discarding its {discarded_bytes}-byte unterminated tail",
            log.path().display()
        ));
    }
}

/// PostToolUse: append a `test.run` span iff the Bash command ran tests. Never
/// blocks.
fn run_record(event: &HookEvent) -> Result<i32> {
    if let Some(span_type) = recorded_span_type(event) {
        let base = resolve_base(event)?;
        let log = LiveLog::for_session(&base, &event.session_id);
        report_repair(&log, log.append(span_type, Map::new())?);
    }
    Ok(0)
}

/// PreToolUse: block the pending write when it targets a policy-protected path
/// (R5), a declared generated artifact (R6), or when no reproduction precedes it
/// (R7). Protected-path and generated-artifact are unconditional; the
/// reproduction gate is skippable by policy. Protected-path is checked first —
/// "may not write at all" outranks "edit the source instead".
fn run_check(event: &HookEvent) -> Result<i32> {
    if !MUTATION_TOOLS.contains(&event.tool_name.as_str()) {
        // Not a guarded mutation; nothing to gate.
        return Ok(0);
    }

    let base = resolve_base(event)?;
    let policy = load_policy(&base)?;
    let targets = write_target(event)
        .map(|raw| policy_write_targets(&base, raw))
        .transpose()?;

    if let (Some(policy), Some(targets)) = (&policy, targets.as_deref()) {
        let compiled = policy.compile()?;
        // R5: protected paths are forbidden outright, regardless of reproduction.
        if let Some(target) = targets.iter().find(|target| compiled.is_protected(target)) {
            return block(&base, event, BlockReason::ProtectedPath(target.clone()));
        }
        // R6: generated artifacts are derived — redirect the agent to the source
        // rather than letting it hand-edit the artifact.
        let rules = generated_rules(policy)?;
        for target in targets {
            if let Decision::Block(reason) = generated_artifact_gate(&rules, target) {
                return block(&base, event, reason);
            }
        }
    }

    // R7: reproduction gate, on unless the policy explicitly disables it.
    let reproduction_required = policy.as_ref().is_none_or(|p| p.reproduction_required);
    if !reproduction_required {
        return allow(&base, event);
    }

    let prior = LiveLog::for_session(&base, &event.session_id).read_spans()?;
    match reproduction_gate(&prior) {
        Decision::Allow => allow(&base, event),
        Decision::Block(reason) => block(&base, event, reason),
    }
}

/// The allow path for a guarded mutation: record the permitted write as a
/// `write.attempt` span carrying its target path, then exit 0 so the tool call
/// proceeds.
///
/// This span records *intent only*. It fires before the tool runs, so it cannot
/// attest that anything landed — the write may still fail, be denied, or be
/// abandoned when the session ends. The held-out ground truth the live corpus
/// accumulates (aoa-d6t.23) comes from the matching `write.committed` span
/// emitted by [`run_outcome`] on the host's success event; see
/// [`SpanType::is_confirmed_mutation`]. Intent is kept anyway because the gap
/// between what an agent tried to write and what it managed to write is itself
/// signal.
fn allow(base: &Path, event: &HookEvent) -> Result<i32> {
    record_write_span(base, event, SpanType::WriteAttempt)?;
    Ok(0)
}

/// Emit the `write.blocked` span, surface the reason on stderr, and return the
/// exit code (2) that signals Claude Code to deny the pending tool call.
fn block(base: &Path, event: &HookEvent, reason: BlockReason) -> Result<i32> {
    let log = LiveLog::for_session(base, &event.session_id);
    let message = reason.to_string();
    report_repair(&log, log.append_with(|seq| blocked_span(seq, reason))?);
    eprint_human(&format!("aoa: blocked {} — {message}", event.tool_name));
    Ok(BLOCK_EXIT_CODE)
}

/// The repo-relative path a write event targets, if any (`file_path` for the
/// edit tools, `notebook_path` for notebooks).
fn write_target(event: &HookEvent) -> Option<&str> {
    event
        .tool_input
        .get("file_path")
        .or_else(|| event.tool_input.get("notebook_path"))
        .and_then(Value::as_str)
}

/// Return every repository-relative spelling relevant to path policy: the
/// lexical hook spelling and the symlink-resolved destination. Matching both
/// prevents an in-repository symlink from hiding either a protected alias or a
/// protected destination.
fn policy_write_targets(base: &Path, raw: &str) -> Result<Vec<String>> {
    let resolved = normalize_write_target(base, raw)?;
    let lexical = normalize_lexically(&hook_candidate(base, Path::new(raw)))?;
    let Some(lexical) = lexical
        .strip_prefix(base)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .and_then(Path::to_str)
    else {
        return Ok(vec![resolved]);
    };

    if lexical == resolved {
        Ok(vec![resolved])
    } else {
        Ok(vec![lexical.to_string(), resolved])
    }
}

/// Where a hook target points before any resolution: absolute spellings stand
/// alone, relative ones hang off the repository root.
fn hook_candidate(base: &Path, raw: &Path) -> PathBuf {
    if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    }
}

/// Resolve a hook target against the canonical repository root and return the
/// repository-relative spelling consumed by policy globs.
///
/// The resolution rule for a not-yet-existing target belongs to
/// [`resolve_canonicalizing`]; what this adds is the repository containment
/// check on its result, which rejects both traversal and symlink escapes.
fn normalize_write_target(base: &Path, raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(anyhow!("hook write target must not be empty"));
    }

    let raw = Path::new(raw);
    let candidate = hook_candidate(base, raw);
    let resolved = resolve_canonicalizing(&candidate).map_err(|source| match source {
        PathTrustError::EscapesRoot { .. } => outside_repository(base, raw),
        other => anyhow!(other),
    })?;

    let relative = resolved
        .strip_prefix(base)
        .map_err(|_| outside_repository(base, raw))?;
    if relative.as_os_str().is_empty() {
        return Err(anyhow!("hook write target resolves to repository root"));
    }
    relative
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("resolved hook write target is not UTF-8: {relative:?}"))
}

fn outside_repository(base: &Path, raw: &Path) -> anyhow::Error {
    anyhow!(
        "hook write target resolves outside repository {}: {raw:?}",
        base.display()
    )
}

/// Load `<base>/aoa-policy.yaml` if it exists, failing loud on a malformed file
/// — a broken policy must not silently disable enforcement.
fn load_policy(base: &Path) -> Result<Option<Policy>> {
    let path = base.join("aoa-policy.yaml");
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            Ok(Some(Policy::from_yaml(&raw).with_context(|| {
                format!("invalid policy at {}", path.display())
            })?))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow!(err)).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Which span (if any) a recorded tool event maps to. Today only the
/// reproduction signal matters, classified by the same detector the offline
/// shim uses so the two paths never diverge.
fn recorded_span_type(event: &HookEvent) -> Option<SpanType> {
    if event.tool_name != "Bash" {
        return None;
    }
    let command = event.tool_input.get("command").and_then(Value::as_str)?;
    bash_runs_tests(command).then_some(SpanType::TestRun)
}

/// Resolve the repository trust root for the hook payload.
///
/// The host normally supplies an absolute project `cwd`; an empty value falls
/// back to the process directory. Canonicalization proves the directory exists
/// and collapses aliases, then the nearest non-symlink `.git` marker pins writes
/// and policy reads to a real repository root rather than an arbitrary payload
/// path. A `.git` file is accepted for linked worktrees.
fn resolve_base(event: &HookEvent) -> Result<PathBuf> {
    let supplied = if event.cwd.is_empty() {
        std::env::current_dir().context("failed to resolve current directory")?
    } else {
        let path = PathBuf::from(&event.cwd);
        if !path.is_absolute() {
            return Err(anyhow!("hook cwd must be absolute: {:?}", event.cwd));
        }
        path
    };
    let canonical = supplied
        .canonicalize()
        .with_context(|| format!("failed to resolve hook cwd {}", supplied.display()))?;
    if !canonical.is_dir() {
        return Err(anyhow!(
            "hook cwd is not a directory: {}",
            canonical.display()
        ));
    }

    let mut nearest_rejection = None;
    for candidate in canonical.ancestors() {
        let marker = candidate.join(".git");
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!(
                    "refusing repository root with symlinked marker at {}",
                    marker.display()
                ));
            }
            Ok(metadata) => match git_candidate_is_root(candidate, metadata.is_dir())? {
                None => return Ok(candidate.to_path_buf()),
                Some(rejection) => {
                    nearest_rejection.get_or_insert((marker, rejection));
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(anyhow!(err))
                    .with_context(|| format!("failed to inspect {}", marker.display()));
            }
        }
    }
    if let Some((marker, rejection)) = nearest_rejection {
        return Err(anyhow!(
            "Git repository validation failed for {}: {rejection}",
            marker.display()
        ));
    }
    Err(anyhow!(
        "hook cwd {} is not inside a Git repository",
        canonical.display()
    ))
}

/// Ask Git to validate the marker and require it to identify this exact
/// canonical directory as the worktree root.
fn git_candidate_is_root(
    candidate: &Path,
    marker_is_dir: bool,
) -> Result<Option<GitCandidateRejection>> {
    git_candidate_is_root_with(candidate, marker_is_dir, &mut git_resolved_path)
}

fn git_candidate_is_root_with(
    candidate: &Path,
    marker_is_dir: bool,
    resolve: &mut impl FnMut(&Path, &'static str) -> Result<GitPathResolution>,
) -> Result<Option<GitCandidateRejection>> {
    let reported = match resolve(candidate, "--show-toplevel")? {
        Ok(path) => path,
        Err(rejection) => {
            return Ok(Some(GitCandidateRejection::Command(rejection)));
        }
    };
    if reported != candidate {
        return Ok(Some(GitCandidateRejection::RootMismatch {
            expected: candidate.to_path_buf(),
            reported,
        }));
    }
    if marker_is_dir {
        return Ok(None);
    }

    let git_dir = match resolve(candidate, "--git-dir")? {
        Ok(path) => path,
        Err(rejection) => {
            return Ok(Some(GitCandidateRejection::Command(rejection)));
        }
    };
    let common_dir = match resolve(candidate, "--git-common-dir")? {
        Ok(path) => path,
        Err(rejection) => {
            return Ok(Some(GitCandidateRejection::Command(rejection)));
        }
    };
    if git_dir.parent() != Some(common_dir.join("worktrees").as_path()) {
        return Ok(Some(GitCandidateRejection::LinkedWorktreeLayout {
            git_dir,
            common_dir,
        }));
    }
    if linked_worktree_points_back(candidate, &git_dir)? {
        Ok(None)
    } else {
        Ok(Some(GitCandidateRejection::BacklinkMismatch {
            candidate: candidate.to_path_buf(),
            git_dir,
        }))
    }
}

type GitPathResolution = std::result::Result<PathBuf, GitCommandRejection>;

#[derive(Debug)]
struct GitCommandRejection {
    field: &'static str,
    status: ExitStatus,
    stderr: String,
}

#[derive(Debug)]
enum GitCandidateRejection {
    Command(GitCommandRejection),
    RootMismatch {
        expected: PathBuf,
        reported: PathBuf,
    },
    LinkedWorktreeLayout {
        git_dir: PathBuf,
        common_dir: PathBuf,
    },
    BacklinkMismatch {
        candidate: PathBuf,
        git_dir: PathBuf,
    },
}

impl fmt::Display for GitCandidateRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(rejection) => rejection.fmt(formatter),
            Self::RootMismatch { expected, reported } => write!(
                formatter,
                "Git check --show-toplevel reported {} instead of candidate {}",
                reported.display(),
                expected.display()
            ),
            Self::LinkedWorktreeLayout {
                git_dir,
                common_dir,
            } => write!(
                formatter,
                "Git linked-worktree layout is invalid: --git-dir reported {}, which is not directly under {}/worktrees from --git-common-dir",
                git_dir.display(),
                common_dir.display()
            ),
            Self::BacklinkMismatch { candidate, git_dir } => write!(
                formatter,
                "linked-worktree backlink at {}/gitdir does not point to {}/.git",
                git_dir.display(),
                candidate.display()
            ),
        }
    }
}

impl fmt::Display for GitCommandRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Git check {} failed with status {}",
            self.field, self.status
        )?;
        if !self.stderr.is_empty() {
            write!(formatter, ": {}", self.stderr)
        } else {
            write!(formatter, " (no stderr)")
        }
    }
}

fn git_resolved_path(candidate: &Path, field: &'static str) -> Result<GitPathResolution> {
    // Git's complete `rev-parse --local-env-vars` set, plus the two discovery
    // controls it omits. Any one of these must describe the candidate itself,
    // never ambient state inherited from the hook host.
    const REPOSITORY_ENV: [&str; 17] = [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ];
    let mut command = Command::new("git");
    for variable in REPOSITORY_ENV {
        command.env_remove(variable);
    }
    let output = command
        .arg("-C")
        .arg(candidate)
        .args(["rev-parse", "--path-format=absolute", field])
        .output()
        .context("failed to run git while validating the hook cwd")?;
    if !output.status.success() {
        return Ok(Err(GitCommandRejection {
            field,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned(),
        }));
    }

    let reported = std::str::from_utf8(&output.stdout)
        .context("git returned a non-UTF-8 repository root")?
        .trim_end();
    let reported = Path::new(reported)
        .canonicalize()
        .with_context(|| format!("failed to resolve Git repository root {reported}"))?;
    Ok(Ok(reported))
}

fn linked_worktree_points_back(candidate: &Path, git_dir: &Path) -> Result<bool> {
    const MAX_BACKLINK_BYTES: u64 = 4096;
    let mut raw = Vec::new();
    File::open(git_dir.join("gitdir"))
        .context("failed to open linked-worktree backlink")?
        .take(MAX_BACKLINK_BYTES + 1)
        .read_to_end(&mut raw)
        .context("failed to read linked-worktree backlink")?;
    if raw.len() as u64 > MAX_BACKLINK_BYTES {
        return Ok(false);
    }
    let backlink = std::str::from_utf8(&raw)
        .context("linked-worktree backlink is not UTF-8")?
        .trim_end();
    let backlink = Path::new(backlink);
    let backlink = if backlink.is_absolute() {
        backlink.to_path_buf()
    } else {
        git_dir.join(backlink)
    };
    Ok(backlink.canonicalize().ok() == candidate.join(".git").canonicalize().ok())
}

/// Merge the enforcement hook entries into an existing `.claude/settings.json`
/// value, idempotently. Re-running produces a byte-identical result: an entry is
/// added only when no hook with the same command string is already registered
/// under its event *with the same matcher*. Pure so `observe --enforce` can test
/// the merge in isolation.
///
/// Fallible on purpose. Every shape this rejects used to be handled silently, in
/// a way that made the module's own upgrade remedy — re-run `observe --enforce` —
/// quietly fail to do anything:
///
/// - a non-object `settings.json` was *replaced*, so the caller then wrote the
///   replacement over the operator's file and destroyed it with no diagnostic;
/// - a non-object `hooks` key panicked, aborting the process instead of
///   reporting a fixable config error;
/// - a non-array event value was skipped, so install reported success while
///   registering nothing.
///
/// All three are hand-edited-config cases, which is exactly when an operator is
/// relying on the tool to tell them the truth.
pub(crate) fn merge_enforce_hooks(mut settings: Value) -> Result<Value> {
    let Some(object) = settings.as_object_mut() else {
        return Err(anyhow!(
            "settings must be a JSON object, found {}",
            json_kind(&settings)
        ));
    };
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(anyhow!(
            "settings key \"hooks\" must be a JSON object, found {}",
            json_kind(hooks)
        ));
    };

    let matcher = mutation_tool_matcher();
    retire_legacy_hooks(hooks);
    // Every command runs the repo-local wrapper, never a bare `aoa`: see
    // `hook_command`. Keep the spelling there, in the generator, so
    // repositories cannot make the decision independently.
    add_hook(hooks, "PostToolUse", "Bash", &hook_command("record"))?;
    add_hook(hooks, "PreToolUse", &matcher, &hook_command("check"))?;
    // One hook per write outcome, each with its own command string. The distinct
    // commands are still required even though `add_hook` now keys on matcher as
    // well: the host runs every group whose matcher fits, so two entries sharing
    // a command would run it twice per tool call and double every span it emits.
    for (event, verb) in [
        ("PostToolUse", "commit"),
        ("PostToolUseFailure", "fail"),
        ("PermissionDenied", "deny"),
    ] {
        add_hook(hooks, event, &matcher, &hook_command(verb))?;
    }
    let aoa = object.entry(AOA_SETTINGS_KEY).or_insert_with(|| json!({}));
    let Some(aoa) = aoa.as_object_mut() else {
        return Err(anyhow!(
            "settings key {AOA_SETTINGS_KEY:?} must be a JSON object, found {}",
            json_kind(aoa)
        ));
    };
    aoa.insert(
        HOOK_VERSION_KEY.to_string(),
        json!(ENFORCE_HOOK_SET_VERSION),
    );
    Ok(settings)
}

/// Drop the hook-set-1 commands (`aoa enforce <verb>`) this installer wrote.
///
/// The merge is otherwise purely additive, which is right for hooks it does not
/// own but wrong for its own superseded ones: leaving them registered means the
/// host keeps running the bare command beside the wrapper, so every tool call
/// still emits the resolution failure the wrapper exists to end, and any repo
/// where `aoa` *is* on PATH records two spans per event. Only the exact command
/// strings this installer has written are removed; anything else an operator
/// added is left alone. Groups emptied by the removal go with them, so a re-run
/// stays byte-stable.
fn retire_legacy_hooks(hooks: &mut Map<String, Value>) {
    const LEGACY_COMMANDS: [&str; 5] = [
        "aoa enforce record",
        "aoa enforce check",
        "aoa enforce commit",
        "aoa enforce fail",
        "aoa enforce deny",
    ];
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            entries.retain(|entry| {
                let command = entry.get("command").and_then(Value::as_str);
                !command.is_some_and(|command| LEGACY_COMMANDS.contains(&command))
            });
        }
        groups.retain(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|entries| !entries.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|groups| !groups.is_empty()));
}

pub(crate) fn enforce_hook_warning(repo: &Path) -> Option<String> {
    let path = repo.join(".claude").join("settings.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            return Some(format!(
                "{} has an unreadable enforce hook stamp ({err}); rerun `aoa observe --enforce`",
                path.display()
            ))
        }
    };
    let settings: Value = match serde_json::from_str(&raw) {
        Ok(settings) => settings,
        Err(_) => {
            return Some(format!(
                "{} has a malformed enforce hook stamp; repair the JSON and rerun `aoa observe --enforce`",
                path.display()
            ))
        }
    };
    let Some(version) = settings
        .get(AOA_SETTINGS_KEY)
        .and_then(Value::as_object)
        .and_then(|aoa| aoa.get(HOOK_VERSION_KEY))
    else {
        return Some(format!(
            "{} is missing the enforce hook stamp (current version {ENFORCE_HOOK_SET_VERSION}); rerun `aoa observe --enforce`",
            path.display()
        ));
    };
    let Some(version) = version.as_u64() else {
        return Some(format!(
            "{} has a malformed enforce hook stamp; rerun `aoa observe --enforce`",
            path.display()
        ));
    };
    match version.cmp(&ENFORCE_HOOK_SET_VERSION) {
        std::cmp::Ordering::Less => Some(format!(
            "{} enforce hooks are behind (installed {version}, current {ENFORCE_HOOK_SET_VERSION}); rerun `aoa observe --enforce`",
            path.display()
        )),
        std::cmp::Ordering::Greater => Some(format!(
            "{} enforce hooks are ahead of this binary (installed {version}, current {ENFORCE_HOOK_SET_VERSION}); upgrade AOA before reinstalling",
            path.display()
        )),
        std::cmp::Ordering::Equal => missing_wrapper_warning(repo),
    }
}

/// Report a current stamp whose wrapper is absent or not executable.
///
/// A stamp says which hook set was installed; it cannot say whether the file
/// those hooks run still exists. Without this check a deleted wrapper leaves
/// the settings looking current while every hook fails, which is the same
/// installed-but-inert reading the wrapper was introduced to end.
fn missing_wrapper_warning(repo: &Path) -> Option<String> {
    let wrapper = repo.join(ENFORCE_WRAPPER_REL);
    let Ok(metadata) = std::fs::symlink_metadata(&wrapper) else {
        return Some(format!(
            "{} is missing; the installed enforce hooks have nothing to run — rerun `aoa observe --enforce`",
            wrapper.display()
        ));
    };
    if !metadata.file_type().is_file() {
        return Some(format!(
            "{} is not a regular file; enforce hooks cannot run — rerun `aoa observe --enforce`",
            wrapper.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Some(format!(
                "{} is not executable; enforce hooks cannot run — rerun `aoa observe --enforce`",
                wrapper.display()
            ));
        }
    }
    None
}

/// Name a JSON value's type for an error message.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Merge the enforcement hooks into `<repo>/.claude/settings.json`, creating the
/// file and its parent if absent. Idempotent: an existing file is parsed,
/// merged, and rewritten, so a re-run that changes nothing is byte-stable.
/// Shared by `observe --enforce` and `policy compile`.
pub(crate) fn install_enforce_hooks(repo: &Path) -> Result<PathBuf> {
    let settings_path = repo.join(".claude").join("settings.json");

    let existing = match std::fs::read_to_string(&settings_path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .with_context(|| format!("{} is not valid JSON", settings_path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(err) => {
            return Err(anyhow!(err))
                .with_context(|| format!("failed to read {}", settings_path.display()))
        }
    };

    // Name the file in the error: `merge_enforce_hooks` is pure and has no path,
    // so without this an operator with a hand-edited config is told the shape is
    // wrong but not which file to open.
    let merged = merge_enforce_hooks(existing).with_context(|| {
        format!(
            "cannot install enforcement hooks into {}",
            settings_path.display()
        )
    })?;

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered =
        serde_json::to_string_pretty(&merged).context("failed to render settings.json")?;
    std::fs::write(&settings_path, format!("{rendered}\n"))
        .with_context(|| format!("failed to write {}", settings_path.display()))?;

    install_enforce_wrapper(repo)?;

    Ok(settings_path)
}

/// Write the wrapper the installed hooks invoke, and make it executable.
///
/// Installing the settings without the wrapper would register hooks pointing at
/// a file that does not exist, which is the failure this whole path exists to
/// remove; the two are written together or the install fails.
fn install_enforce_wrapper(repo: &Path) -> Result<()> {
    let wrapper_path = repo.join(ENFORCE_WRAPPER_REL);
    if let Some(parent) = wrapper_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&wrapper_path, ENFORCE_WRAPPER_SCRIPT)
        .with_context(|| format!("failed to write {}", wrapper_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper_path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to make {} executable", wrapper_path.display()))?;
    }
    Ok(())
}

/// Ensure `hooks[event]` contains a matcher group running `command`.
///
/// Idempotent on the shape this installs: an entry already registered under the
/// same matcher is left exactly as it is, so a re-run is byte-stable.
///
/// The matcher is part of the identity, and both ways of getting that wrong are
/// errors rather than guesses. Keying on the command alone (the previous
/// behaviour) meant an entry registered under *any* matcher suppressed the
/// install, so a command pre-seeded under an unrelated matcher silently left the
/// hook uninstalled while install still reported success. Installing a second
/// group whenever the matcher differs would be worse: the host runs every group
/// whose matcher fits, so the command would fire twice per tool call and write
/// two spans for every one write. Neither is recoverable by the tool, so it says
/// what it found and stops.
fn add_hook(
    hooks: &mut Map<String, Value>,
    event: &str,
    matcher: &str,
    command: &str,
) -> Result<()> {
    let groups = hooks.entry(event).or_insert_with(|| json!([]));
    let Some(groups) = groups.as_array_mut() else {
        return Err(anyhow!(
            "hook event \"{event}\" must be an array, found {}",
            json_kind(groups)
        ));
    };

    let registered_matcher = groups.iter().find_map(|group| {
        let runs_command = group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|inner| {
                inner
                    .iter()
                    .any(|h| h.get("command").and_then(Value::as_str) == Some(command))
            });
        runs_command.then(|| group.get("matcher").and_then(Value::as_str))
    });

    match registered_matcher {
        Some(Some(found)) if found == matcher => Ok(()),
        // A group with no usable matcher still can't be reconciled, but naming it
        // as `""` would read as an entry that matches the empty string rather
        // than one that is missing the key.
        Some(found) => Err(anyhow!(
            "hook event \"{event}\" already runs \"{command}\" under {}, but it \
             must run under matcher \"{matcher}\". Remove or correct that entry \
             and re-run.",
            found.map_or_else(
                || "a group with no matcher".to_string(),
                |m| format!("matcher \"{m}\"")
            )
        )),
        None => {
            groups.push(json!({
                "matcher": matcher,
                "hooks": [{ "type": "command", "command": command }],
            }));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(tool: &str, command: Option<&str>) -> HookEvent {
        let mut tool_input = Map::new();
        if let Some(c) = command {
            tool_input.insert("command".to_string(), Value::String(c.to_string()));
        }
        HookEvent {
            session_id: "sess-1".to_string(),
            tool_name: tool.to_string(),
            tool_input,
            cwd: String::new(),
        }
    }

    fn init_git_repo(path: &Path) {
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .arg(path)
            .status()
            .expect("git is available for repository-boundary tests");
        assert!(status.success(), "git init failed for test fixture");
    }

    #[test]
    fn records_test_run_only_for_test_commands() {
        assert_eq!(
            recorded_span_type(&event("Bash", Some("cargo test --all"))),
            Some(SpanType::TestRun)
        );
        assert_eq!(recorded_span_type(&event("Bash", Some("ls -la"))), None);
        assert_eq!(recorded_span_type(&event("Write", None)), None);
    }

    #[test]
    fn write_target_normalization_accepts_supported_in_repository_shapes() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let absolute = base.join(".github/workflows/ci.yml");

        for raw in [
            absolute.to_str().unwrap(),
            "./.github/workflows/ci.yml",
            "src/../.github/workflows/ci.yml",
        ] {
            assert_eq!(
                normalize_write_target(&base, raw).unwrap(),
                ".github/workflows/ci.yml"
            );
        }
    }

    #[test]
    fn write_target_normalization_rejects_invalid_boundaries() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();

        assert!(normalize_write_target(&base, "../outside.rs")
            .unwrap_err()
            .to_string()
            .contains("outside repository"));
        assert!(normalize_write_target(&base, "").is_err());
        assert!(normalize_write_target(&base, ".").is_err());
    }

    #[test]
    fn resolve_base_rejects_an_existing_non_repository_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let mut e = event("Write", None);
        e.cwd = dir.path().to_string_lossy().into_owned();

        let err = resolve_base(&e).expect_err("an arbitrary directory is not a trust root");
        let message = err.to_string();
        assert!(message.contains("--show-toplevel"), "{message}");
        assert!(message.contains("status"), "{message}");
        assert!(message.contains("not a git repository"), "{message}");
        assert!(!dir.path().join(".aoa").exists());
    }

    #[test]
    fn git_candidate_reports_show_toplevel_failure() {
        let (_fixture, candidate) = candidate_fixture();
        let message = rejection_message(
            &candidate,
            true,
            vec![(
                "--show-toplevel",
                rejected_git_path(
                    "--show-toplevel",
                    "fatal: detected dubious ownership in repository",
                ),
            )],
        );
        assert!(message.contains("--show-toplevel"), "{message}");
        assert!(message.contains("dubious ownership"), "{message}");
    }

    #[test]
    fn git_candidate_reports_mismatched_toplevel() {
        let (_fixture, candidate) = candidate_fixture();
        let other = candidate.join("other");
        std::fs::create_dir(&other).unwrap();
        let message = rejection_message(
            &candidate,
            true,
            vec![("--show-toplevel", Ok(other.clone()))],
        );
        assert!(message.contains("--show-toplevel"), "{message}");
        assert!(message.contains(&other.display().to_string()), "{message}");
    }

    #[test]
    fn git_candidate_reports_git_dir_failure() {
        let (_fixture, candidate) = candidate_fixture();
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                (
                    "--git-dir",
                    rejected_git_path("--git-dir", "fatal: corrupt config"),
                ),
            ],
        );
        assert!(message.contains("--git-dir"), "{message}");
        assert!(message.contains("corrupt config"), "{message}");
    }

    #[test]
    fn git_candidate_reports_common_dir_failure() {
        let (_fixture, candidate) = candidate_fixture();
        let git_dir = candidate.join("admin/worktrees/fixture");
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                (
                    "--git-common-dir",
                    rejected_git_path("--git-common-dir", "fatal: invalid common directory"),
                ),
            ],
        );
        assert!(message.contains("--git-common-dir"), "{message}");
        assert!(message.contains("invalid common directory"), "{message}");
    }

    #[test]
    fn git_candidate_reports_invalid_linked_worktree_layout() {
        let (_fixture, candidate) = candidate_fixture();
        let git_dir = candidate.join("admin/worktrees/fixture");
        let common_dir = candidate.join("different-admin");
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                ("--git-common-dir", Ok(common_dir.clone())),
            ],
        );
        assert!(message.contains("linked-worktree layout"), "{message}");
        assert!(
            message.contains(&git_dir.display().to_string()),
            "{message}"
        );
        assert!(
            message.contains(&common_dir.display().to_string()),
            "{message}"
        );
    }

    #[test]
    fn git_candidate_reports_mismatched_linked_worktree_backlink() {
        let (_fixture, candidate) = candidate_fixture();
        let common_dir = candidate.join("admin");
        let git_dir = common_dir.join("worktrees/fixture");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(candidate.join(".git"), "gitdir: ignored\n").unwrap();
        std::fs::write(
            git_dir.join("gitdir"),
            candidate
                .join("missing-marker")
                .to_string_lossy()
                .as_bytes(),
        )
        .unwrap();
        let message = rejection_message(
            &candidate,
            false,
            vec![
                ("--show-toplevel", Ok(candidate.clone())),
                ("--git-dir", Ok(git_dir.clone())),
                ("--git-common-dir", Ok(common_dir)),
            ],
        );
        assert!(message.contains("backlink"), "{message}");
        assert!(
            message.contains(&git_dir.display().to_string()),
            "{message}"
        );
    }

    fn candidate_fixture() -> (tempfile::TempDir, PathBuf) {
        let fixture = tempfile::tempdir().unwrap();
        let candidate = fixture.path().canonicalize().unwrap();
        (fixture, candidate)
    }

    fn rejection_message(
        candidate: &Path,
        marker_is_dir: bool,
        resolutions: Vec<(&'static str, GitPathResolution)>,
    ) -> String {
        let mut resolutions = resolutions.into_iter();
        let mut resolve = |_: &Path, field| {
            let (expected, resolution) = resolutions.next().expect("unexpected Git check");
            assert_eq!(field, expected);
            Ok(resolution)
        };
        git_candidate_is_root_with(candidate, marker_is_dir, &mut resolve)
            .unwrap()
            .expect("candidate must be rejected")
            .to_string()
    }

    fn rejected_git_path(field: &'static str, stderr: &str) -> GitPathResolution {
        let status = Command::new("git")
            .arg("--definitely-not-a-real-option")
            .output()
            .unwrap()
            .status;
        Err(GitCommandRejection {
            field,
            status,
            stderr: stderr.to_string(),
        })
    }

    #[test]
    fn resolve_base_accepts_a_linked_worktree_git_file() {
        let fixture = tempfile::tempdir().unwrap();
        let main = fixture.path().join("main");
        let worktree = fixture.path().join("worktree");
        init_git_repo(&main);
        let status = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args([
                "-c",
                "user.name=AOA Test",
                "-c",
                "user.email=aoa@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "worktree fixture commit must succeed");
        let status = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&worktree)
            .status()
            .unwrap();
        assert!(status.success(), "linked worktree must initialize");
        let mut e = event("Write", None);
        e.cwd = worktree.to_string_lossy().into_owned();

        assert_eq!(resolve_base(&e).unwrap(), worktree);
    }

    #[test]
    fn resolve_base_accepts_relative_linked_worktree_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let main = fixture.path().join("main");
        let worktree = fixture.path().join("worktree");
        init_git_repo(&main);
        let status = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args([
                "-c",
                "user.name=AOA Test",
                "-c",
                "user.email=aoa@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "worktree fixture commit must succeed");
        let status = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&worktree)
            .status()
            .unwrap();
        assert!(status.success(), "linked worktree must initialize");

        let admin_dir = main.join(".git/worktrees/worktree");
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../main/.git/worktrees/worktree\n",
        )
        .unwrap();
        std::fs::write(admin_dir.join("gitdir"), "../../../../worktree/.git\n").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&worktree)
                .args(["rev-parse", "--is-inside-work-tree"])
                .output()
                .unwrap()
                .status
                .success(),
            "Git must accept the relative linked-worktree metadata fixture"
        );

        let mut e = event("Write", None);
        e.cwd = worktree.to_string_lossy().into_owned();
        assert_eq!(resolve_base(&e).unwrap(), worktree);
    }

    #[test]
    fn resolve_base_ignores_a_nested_marker_redirecting_to_its_parent() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let nested = repo.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            format!("gitdir: {}\n", repo.path().join(".git").display()),
        )
        .unwrap();
        let mut e = event("Write", None);
        e.cwd = nested.to_string_lossy().into_owned();

        assert_eq!(resolve_base(&e).unwrap(), repo.path());
    }

    /// Re-merging an already-installed config must be byte-stable: every entry is
    /// present under the matcher `add_hook` keys on, so the second pass finds
    /// them all and changes nothing.
    #[test]
    fn merge_enforce_hooks_is_idempotent() {
        let once = merge_enforce_hooks(json!({})).expect("fresh settings merge");
        let twice = merge_enforce_hooks(once.clone()).expect("re-merging an installed config");
        assert_eq!(once, twice, "second merge must be a no-op");

        // Pinned as a wire contract: this is the alternation syntax Claude Code
        // matchers use, and it is derived rather than written out.
        let matcher = mutation_tool_matcher();
        assert_eq!(matcher, "Write|Edit|MultiEdit|NotebookEdit");

        // PostToolUse carries two entries under different matchers: the Bash
        // test recorder and the mutation-tool commit recorder. They must have
        // distinct command strings — one command under two matchers is the
        // conflict `add_hook` rejects, so sharing one would fail the install.
        let post = once["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["hooks"][0]["command"], hook_command("record"));
        assert_eq!(post[0]["matcher"], "Bash");
        assert_eq!(post[1]["hooks"][0]["command"], hook_command("commit"));
        assert_eq!(post[1]["matcher"], matcher);

        let pre = &once["hooks"]["PreToolUse"];
        assert_eq!(pre[0]["hooks"][0]["command"], hook_command("check"));

        for (event, command) in [
            ("PostToolUseFailure", hook_command("fail")),
            ("PermissionDenied", hook_command("deny")),
        ] {
            let group = once["hooks"][event].as_array().unwrap();
            assert_eq!(group.len(), 1, "{event} registers exactly one hook");
            assert_eq!(group[0]["hooks"][0]["command"], command);
            assert_eq!(group[0]["matcher"], matcher);
        }
    }

    /// Every write outcome the host can report has somewhere to be recorded.
    /// Without the full set, an outcome silently goes unobserved and its writes
    /// look like abandoned attempts.
    #[test]
    fn every_write_outcome_has_a_registered_hook() {
        let merged = merge_enforce_hooks(json!({})).expect("fresh settings merge");
        let matcher = mutation_tool_matcher();
        let commands: Vec<String> = ["PostToolUse", "PostToolUseFailure", "PermissionDenied"]
            .iter()
            .filter_map(|event| merged["hooks"][event].as_array())
            .flatten()
            .filter(|g| g["matcher"] == matcher)
            .map(|g| g["hooks"][0]["command"].as_str().unwrap().to_string())
            .collect();

        assert_eq!(
            commands,
            [
                hook_command("commit"),
                hook_command("fail"),
                hook_command("deny")
            ]
        );
    }

    /// Upgrading a hook-set-1 install must retire the bare commands, not sit
    /// beside them. Left registered they keep failing to resolve on every tool
    /// call — the error wall this change removes — and double every span in the
    /// repos where they do resolve.
    #[test]
    fn upgrading_retires_the_bare_command_hook_set() {
        let installed_v1 = json!({
            "aoa": { "enforce_hook_set_version": 1 },
            "hooks": {
                "PostToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "aoa enforce record" }] },
                    { "matcher": mutation_tool_matcher(), "hooks": [{ "type": "command", "command": "aoa enforce commit" }] },
                ],
                "PreToolUse": [
                    { "matcher": mutation_tool_matcher(), "hooks": [{ "type": "command", "command": "aoa enforce check" }] },
                ],
            }
        });
        let upgraded = merge_enforce_hooks(installed_v1).expect("upgrade from hook set 1");

        let rendered = serde_json::to_string(&upgraded).unwrap();
        assert!(
            !rendered.contains("\"aoa enforce"),
            "no bare command may survive the upgrade: {rendered}"
        );
        assert_eq!(upgraded["aoa"][HOOK_VERSION_KEY], ENFORCE_HOOK_SET_VERSION);
        // Exactly one entry per event/matcher pair: retired, then reinstalled.
        assert_eq!(
            upgraded["hooks"]["PostToolUse"].as_array().unwrap().len(),
            2
        );
        assert_eq!(upgraded["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    /// Retiring must not reach past the commands this installer wrote, and a
    /// group it empties must go rather than linger as an empty shell that a
    /// re-run would then diff against.
    #[test]
    fn retiring_leaves_other_hooks_and_drops_the_groups_it_empties() {
        let mut hooks = json!({
            "PostToolUse": [
                { "matcher": "Bash", "hooks": [
                    { "command": "aoa enforce record" },
                    { "command": "my-own-recorder" },
                ]},
                { "matcher": "Read", "hooks": [{ "command": "aoa enforce check" }] },
            ],
            "SessionStart": [{ "hooks": [{ "command": "aoa observe" }] }],
        });
        retire_legacy_hooks(hooks.as_object_mut().unwrap());

        let post = hooks["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1, "the emptied Read group is dropped");
        assert_eq!(post[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(post[0]["hooks"][0]["command"], "my-own-recorder");
        // `aoa observe` is not one of the retired commands.
        assert_eq!(
            hooks["SessionStart"][0]["hooks"][0]["command"],
            "aoa observe"
        );
    }

    #[test]
    fn merge_preserves_unrelated_existing_settings_and_hooks() {
        let existing = json!({
            "model": "claude-opus-4-8",
            "hooks": {
                "PostToolUse": [
                    { "matcher": "Read", "hooks": [{ "type": "command", "command": "log-read" }] }
                ]
            }
        });
        let merged = merge_enforce_hooks(existing).expect("merge into existing settings");
        assert_eq!(merged["model"], "claude-opus-4-8");
        // Existing Read hook retained, our Bash and mutation hooks added
        // alongside it.
        let post = merged["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 3);
        for command in [
            "log-read".to_string(),
            hook_command("record"),
            hook_command("commit"),
        ] {
            assert!(
                post.iter().any(|g| g["hooks"][0]["command"] == command),
                "{command} missing from merged PostToolUse hooks"
            );
        }
    }

    /// A malformed config must be reported, never worked around. Each of these
    /// shapes used to be swallowed in a way that left the hooks uninstalled while
    /// `observe --enforce` still exited 0 — so the module's own upgrade remedy
    /// ("re-run `aoa observe --enforce`") could not fix the repos that needed it,
    /// and `held_out_edits` stayed permanently empty with no diagnostic.
    #[test]
    fn malformed_settings_are_reported_not_silently_accepted() {
        // Was a panic: `entry()` returns the existing value, so a non-object
        // `hooks` reached an `.expect` and aborted the process.
        let err = merge_enforce_hooks(json!({ "hooks": [] })).unwrap_err();
        assert!(
            err.to_string().contains("\"hooks\""),
            "error must name the offending key, got: {err}"
        );

        // Was silent data loss: a non-object settings.json was replaced wholesale
        // and the caller then wrote the replacement over the operator's file.
        for hostile in [json!([]), json!("hooks"), json!(null)] {
            assert!(
                merge_enforce_hooks(hostile.clone()).is_err(),
                "{hostile} must be rejected, not replaced"
            );
        }

        // Was a silent skip: a non-array event value returned early.
        let err = merge_enforce_hooks(json!({ "hooks": { "PostToolUse": {} } })).unwrap_err();
        assert!(
            err.to_string().contains("PostToolUse"),
            "error must name the offending event, got: {err}"
        );
    }

    /// Keying dedupe on the command alone meant an entry pre-seeded under any
    /// unrelated matcher suppressed the install entirely, so the mutation hooks
    /// were never registered and install still reported success. Installing a
    /// duplicate group instead would make the host run the command twice per
    /// tool call, so the conflict is reported rather than resolved.
    #[test]
    fn a_command_registered_under_the_wrong_matcher_is_a_loud_conflict() {
        let seeded = json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": hook_command("commit") }],
                }]
            }
        });
        let err = merge_enforce_hooks(seeded).unwrap_err();
        let message = err.to_string();
        for expected in [
            hook_command("commit"),
            "Bash".to_string(),
            mutation_tool_matcher(),
        ] {
            assert!(
                message.contains(&expected),
                "conflict must name {expected}, got: {message}"
            );
        }
    }

    /// A group carrying the command but no `matcher` key is still unreconcilable,
    /// and must say so as a *missing* matcher. Rendering it as `""` would read as
    /// a group matching the empty string, sending the operator looking for an
    /// entry that isn't there.
    #[test]
    fn a_command_registered_without_a_matcher_names_the_absence() {
        let seeded = json!({
            "hooks": {
                "PostToolUse": [{
                    "hooks": [{ "type": "command", "command": hook_command("commit") }],
                }]
            }
        });
        let message = merge_enforce_hooks(seeded).unwrap_err().to_string();
        assert!(
            message.contains("a group with no matcher"),
            "must name the absence rather than an empty matcher, got: {message}"
        );
        assert!(
            !message.contains("matcher \"\""),
            "must not render the missing key as an empty matcher, got: {message}"
        );
    }

    #[test]
    fn check_posture_converts_a_panic_to_the_block_exit_code() {
        let code = run_with_failure_posture(EnforceCommand::Check, || {
            panic!("intentional write-gate panic")
        })
        .unwrap();
        assert_eq!(code, BLOCK_EXIT_CODE);
    }

    #[test]
    fn non_gating_posture_does_not_swallow_a_panic() {
        let panic = std::panic::catch_unwind(|| {
            let _ = run_with_failure_posture(EnforceCommand::Record, || {
                panic!("intentional recorder panic")
            });
        });
        assert!(
            panic.is_err(),
            "non-gating hooks must retain normal panic behavior"
        );
    }
}
