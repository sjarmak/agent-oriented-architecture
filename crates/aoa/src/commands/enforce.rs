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
//! in [`aoa_enforce::live_log`], beside the gates that decide over it, and the
//! trust root every write is judged against comes from
//! [`resolve_repository_root`], beside the containment checks that measure
//! against it. What is left here is the hook shape: read the payload, dispatch,
//! render.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};

use aoa_audit::{HookScope, BLOCK_EXIT_CODE};
use aoa_codeprobe_shim::bash_runs_tests;
use aoa_enforce::{
    blocked_span, generated_artifact_gate, reproduction_gate, BlockReason, Decision, LiveLog,
    TornTailRepair,
};
use aoa_path_trust::{read_regular_file_nofollow, resolve_repository_root};
use aoa_policy::Policy;
use aoa_trace::SpanType;

mod install;
mod scope;

pub(crate) use install::{enforce_hook_warning, install_enforce_hooks};
use scope::{
    governed_write, scope_of_spellings, scope_under, write_candidate, write_target, WriteScope,
};

use crate::cli::{EnforceArgs, EnforceCommand};
use crate::commands::generated::generated_rules;
use crate::output::eprint_human;

const POLICY_ROOT_ATTRIBUTE: &str = "policy_root";
const POLICY_FILE: &str = "aoa-policy.yaml";

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
    if !HookScope::Mutation.selects(&event.tool_name) {
        return Ok(0);
    }
    if let Some(candidate) = write_candidate(event)? {
        let base = resolve_base(event)?;
        if matches!(scope_under(&base, &candidate)?, WriteScope::Outside) {
            return Ok(0);
        }
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
    if !HookScope::Mutation.selects(&event.tool_name) {
        // Not a guarded mutation; nothing to gate.
        return Ok(0);
    }

    let candidate = write_candidate(event)?;
    let base = resolve_base(event)?;
    let mut inside_base = false;
    let mut reproduction_root = None;
    let governed = governed_write(&base, candidate.as_deref())?;
    for root in governed.roots {
        let sessions_own = root == base;
        let targets = if governed.spellings.is_empty() {
            None
        } else {
            match scope_of_spellings(&root, &governed.spellings)? {
                WriteScope::Outside => continue,
                WriteScope::Inside(targets) => Some(targets),
            }
        };
        inside_base |= sessions_own;
        let policy = load_policy(&root)?;
        if let (Some(policy), Some(targets)) = (&policy, targets.as_deref()) {
            if let Some(reason) = path_refusal(policy, targets)? {
                return block(&base, event, &root, reason);
            }
        }
        let reproduction_required = match &policy {
            Some(policy) => policy.reproduction_required,
            None => sessions_own,
        };
        if reproduction_required && reproduction_root.is_none() {
            reproduction_root = Some(root);
        }
    }

    let Some(root) = reproduction_root else {
        return allow(&base, event, inside_base);
    };
    let prior = LiveLog::for_session(&base, &event.session_id).read_spans()?;
    match reproduction_gate(&prior) {
        Decision::Allow => allow(&base, event, inside_base),
        Decision::Block(reason) => block(&base, event, &root, reason),
    }
}

fn path_refusal(policy: &Policy, targets: &[String]) -> Result<Option<BlockReason>> {
    let compiled = policy.compile()?;
    if let Some(target) = targets.iter().find(|target| compiled.is_protected(target)) {
        return Ok(Some(BlockReason::ProtectedPath(target.clone())));
    }
    let rules = generated_rules(policy)?;
    Ok(targets
        .iter()
        .find_map(|target| match generated_artifact_gate(&rules, target) {
            Decision::Block(reason) => Some(reason),
            Decision::Allow => None,
        }))
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
fn allow(base: &Path, event: &HookEvent, inside_base: bool) -> Result<i32> {
    if inside_base {
        record_write_span(base, event, SpanType::WriteAttempt)?;
    }
    Ok(0)
}

/// Emit the `write.blocked` span, surface the reason on stderr, and return the
/// exit code (2) that signals Claude Code to deny the pending tool call.
fn block(base: &Path, event: &HookEvent, root: &Path, reason: BlockReason) -> Result<i32> {
    let log = LiveLog::for_session(base, &event.session_id);
    let message = reason.to_string();
    let policy_root = root.to_string_lossy().into_owned();
    report_repair(
        &log,
        log.append_with(|seq| {
            let mut span = blocked_span(seq, reason);
            span.attributes.insert(
                POLICY_ROOT_ATTRIBUTE.to_string(),
                Value::String(policy_root.clone()),
            );
            span
        })?,
    );
    eprint_human(&format!(
        "aoa: blocked {} — {message} (enforced for {policy_root})",
        event.tool_name
    ));
    Ok(BLOCK_EXIT_CODE)
}

fn load_policy(base: &Path) -> Result<Option<Policy>> {
    let path = base.join(POLICY_FILE);
    read_regular_file_nofollow(base, POLICY_FILE)
        .with_context(|| format!("cannot read policy at {}", path.display()))?
        .map(|raw| {
            Policy::from_yaml(&raw).with_context(|| format!("invalid policy at {}", path.display()))
        })
        .transpose()
}

/// Which span (if any) a recorded tool event maps to. Today only the
/// reproduction signal matters, classified by the same detector the offline
/// shim uses so the two paths never diverge.
fn recorded_span_type(event: &HookEvent) -> Option<SpanType> {
    if !HookScope::Bash.selects(&event.tool_name) {
        return None;
    }
    let command = event.tool_input.get("command").and_then(Value::as_str)?;
    bash_runs_tests(command).then_some(SpanType::TestRun)
}

/// Resolve the repository trust root for the hook payload.
///
/// Reading `event.cwd` is the only hook-shaped part of the answer, so it is the
/// only part left here: the host normally supplies an absolute project path, and
/// an empty value falls back to the process directory. Everything the root is
/// *trusted* for belongs to [`resolve_repository_root`], beside the containment
/// checks measured against it.
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
    Ok(resolve_repository_root(&supplied)?)
}

#[cfg(test)]
mod tests {
    use super::scope::{init_git_repo, repositories_enclosing};
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

    #[test]
    fn records_test_run_only_for_test_commands() {
        assert_eq!(
            recorded_span_type(&event("Bash", Some("cargo test --all"))),
            Some(SpanType::TestRun)
        );
        assert_eq!(recorded_span_type(&event("Bash", Some("ls -la"))), None);
        assert_eq!(recorded_span_type(&event("Write", None)), None);
    }

    /// The adapter's own rule, and the only one it still owns: a payload path
    /// that is not absolute would resolve against the process directory.
    #[test]
    fn resolve_base_rejects_a_relative_hook_cwd() {
        let mut e = event("Write", None);
        e.cwd = "relative/project".to_string();

        let err = resolve_base(&e).expect_err("a relative hook cwd names no trust root");
        assert!(err.to_string().contains("must be absolute"), "{err}");
    }

    #[test]
    fn resolve_base_resolves_the_hook_cwd_to_the_repository_root() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let nested = repo.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let mut e = event("Write", None);
        e.cwd = nested.to_string_lossy().into_owned();

        assert_eq!(
            resolve_base(&e).unwrap(),
            repo.path().canonicalize().unwrap()
        );
    }

    /// A refusal from the trust-root resolver has to reach the hook, not be
    /// softened into a usable base along the way.
    #[test]
    fn resolve_base_surfaces_a_refused_trust_root() {
        let outside = tempfile::tempdir().unwrap();
        if let Some(enclosing) = repositories_enclosing(outside.path()).into_iter().next() {
            eprintln!(
                "SKIP (docs/adr/0004-environment-dependent-test-skips.md): {} encloses the \
                 temp root, so no directory under it is outside a repository",
                enclosing.display()
            );
            return;
        }
        let mut e = event("Write", None);
        e.cwd = outside.path().to_string_lossy().into_owned();

        let err = resolve_base(&e).expect_err("a directory in no repository has no trust root");
        assert!(
            err.to_string().contains("not inside a Git repository"),
            "{err}"
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
