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
use serde_json::{json, Map, Value};

use aoa_audit::{
    hook_command, hook_set_defect, AOA_SETTINGS_KEY, BLOCK_EXIT_CODE, ENFORCE_HOOK_SET,
    ENFORCE_HOOK_SET_VERSION, ENFORCE_WRAPPER_REL, HOOK_VERSION_KEY, SETTINGS_REL,
};
use aoa_codeprobe_shim::bash_runs_tests;
use aoa_enforce::{
    blocked_span, generated_artifact_gate, reproduction_gate, BlockReason, Decision, LiveLog,
    TornTailRepair,
};
use aoa_policy::Policy;
use aoa_trace::{
    normalize_lexically, resolve_canonicalizing, resolve_repository_root, PathTrustError, SpanType,
};

use crate::cli::{EnforceArgs, EnforceCommand};
use crate::commands::generated::generated_rules;
use crate::output::eprint_human;

/// The tools whose writes the gate guards. A pending call to any of these is a
/// mutation and must be preceded by a reproduction (`test.run`) span.
const MUTATION_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// The wrapper's contents, held here so the installer and the drift test read
/// one source rather than two copies that can disagree.
const ENFORCE_WRAPPER_SCRIPT: &str = include_str!("enforce_hook.sh");

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
    if let Some(raw) = write_target(event) {
        let base = resolve_base(event)?;
        // A write landing outside this repository is not this repository's
        // history. Recording it would put a foreign path in the live log the
        // gate reads and the held-out corpus mines, and would show up on the
        // liveness surface as this plane enforcing something.
        if matches!(write_scope(&base, raw)?, WriteScope::Outside) {
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
///
/// Every one of those policies is about *this* repository, so the target's
/// scope is settled before any of them runs. The hook matcher cannot express
/// paths — it fires on `Write|Edit|MultiEdit|NotebookEdit` whatever they target
/// — so if this function did not discriminate, the gate's path domain would be
/// the whole machine: a session in this checkout could not write a note, a
/// scratch file, or a report anywhere else until it ran a test here
/// (aoa-7g14y.1). An over-broad gate is worse than a narrow one, because the
/// natural workaround is to route the write through Bash, which defeats the
/// gate for in-repo paths too.
fn run_check(event: &HookEvent) -> Result<i32> {
    if !MUTATION_TOOLS.contains(&event.tool_name.as_str()) {
        // Not a guarded mutation; nothing to gate.
        return Ok(0);
    }

    let base = resolve_base(event)?;
    let targets = match write_target(event)
        .map(|raw| write_scope(&base, raw))
        .transpose()?
    {
        // Out of scope entirely: allowed without consulting the policy, without
        // the reproduction gate, and — deliberately — without a span. Asking
        // this gate about a foreign path must not manufacture the `.aoa/traces`
        // tree whose contents are read as evidence that this plane runs.
        Some(WriteScope::Outside) => return Ok(0),
        Some(WriteScope::Inside(targets)) => Some(targets),
        None => None,
    };
    let policy = load_policy(&base)?;

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

/// Whether a pending write lands inside the enforcing repository, and if so
/// under which repository-relative spellings the path policies must match it.
#[derive(Debug, PartialEq, Eq)]
enum WriteScope {
    /// Inside: every repository-relative spelling relevant to path policy — the
    /// lexical hook spelling and the symlink-resolved destination. Matching
    /// both prevents an in-repository symlink from hiding either a protected
    /// alias or a protected destination.
    Inside(Vec<String>),
    /// Outside: neither spelling lands in the repository, so no policy this
    /// repository declares has anything to say about the write.
    Outside,
}

/// Classify a hook target against the canonical repository root.
///
/// A target counts as inside when *either* resolution lands under `base`, and
/// the asymmetry is load-bearing in both directions:
///
/// - Resolved-inside catches the `../`-relative and symlinked spellings that
///   walk back into the repository. Containment has to be decided on the
///   resolved path or the check is a string comparison anyone can spell around.
/// - Lexical-inside keeps a repository-local symlink that points out of the
///   tree in scope. Deciding on the resolved path alone would turn such an
///   alias into a way to name any protected path and have the gate wave it
///   through, which is the R5 alias hole the two-spelling match exists to
///   close.
///
/// Only a target outside by both readings is out of scope. Resolution failures
/// other than containment still propagate, so `check` keeps denying on them.
fn write_scope(base: &Path, raw: &str) -> Result<WriteScope> {
    if raw.is_empty() {
        return Err(anyhow!("hook write target must not be empty"));
    }
    let candidate = hook_candidate(base, Path::new(raw));
    let lexical = contained(base, normalize_lexically(&candidate))?;
    let resolved = contained(base, resolve_canonicalizing(&candidate))?;

    Ok(match (lexical, resolved) {
        (None, None) => WriteScope::Outside,
        (Some(lexical), Some(resolved)) if lexical == resolved => {
            WriteScope::Inside(vec![resolved])
        }
        (Some(lexical), Some(resolved)) => WriteScope::Inside(vec![lexical, resolved]),
        (Some(only), None) | (None, Some(only)) => WriteScope::Inside(vec![only]),
    })
}

/// The repository-relative spelling of one resolution of a hook target, or
/// `None` when that resolution lands outside the repository.
///
/// [`PathTrustError::EscapesRoot`] — a `..` chain walking off the filesystem
/// root — is outside by construction, not a failure. Every other error is a
/// real failure and propagates.
fn contained(base: &Path, resolution: Result<PathBuf, PathTrustError>) -> Result<Option<String>> {
    let path = match resolution {
        Ok(path) => path,
        Err(PathTrustError::EscapesRoot { .. }) => return Ok(None),
        Err(other) => return Err(anyhow!(other)),
    };
    let Ok(relative) = path.strip_prefix(base) else {
        return Ok(None);
    };
    if relative.as_os_str().is_empty() {
        return Err(anyhow!("hook write target resolves to repository root"));
    }
    relative
        .to_str()
        .map(|relative| Some(relative.to_owned()))
        .ok_or_else(|| anyhow!("resolved hook write target is not UTF-8: {relative:?}"))
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
    // The event/verb set and the command spelling both come from `aoa-audit`,
    // which is also what reads them back: a hook this installer writes cannot be
    // one the plane check does not recognise. `record` observes Bash test runs;
    // every other verb observes the mutation tools.
    //
    // Every verb keeps its own command string. Distinct commands are still
    // required even though `add_hook` keys on matcher as well: the host runs
    // every group whose matcher fits, so two entries sharing a command would run
    // it twice per tool call and double every span it emits.
    for (event, verb) in ENFORCE_HOOK_SET {
        let scope = if verb == "record" { "Bash" } else { &matcher };
        add_hook(hooks, event, scope, &hook_command(verb))?;
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

/// Every command string this installer has written for `verb` and since
/// superseded.
///
/// Hook set 1 ran a bare `aoa`, which enforced only where the binary happened to
/// be on the host's PATH. Hook set 2 named the wrapper through a `.`-defaulted
/// `CLAUDE_PROJECT_DIR`, which resolved by whatever cwd the host used and exited
/// 127 — a non-blocking warning — from anywhere else. Both are kept here rather
/// than deleted alongside the code that wrote them: a repo installed by an older
/// `aoa` still has them registered, and they are removable only by the exact
/// string that put them there.
fn superseded_hook_commands(verb: &str) -> [String; 2] {
    [
        format!("aoa enforce {verb}"),
        format!("\"${{CLAUDE_PROJECT_DIR:-.}}\"/{ENFORCE_WRAPPER_REL} {verb}"),
    ]
}

/// Drop the superseded commands this installer wrote in an earlier hook set.
///
/// The merge is otherwise purely additive, which is right for hooks it does not
/// own but wrong for its own superseded ones: leaving them registered means the
/// host keeps running the old command beside the current one, so every tool call
/// still emits the failure the current form exists to end, and any repo where
/// the old form *does* resolve records two spans per event. Only the exact
/// command strings this installer has written are removed; anything else an
/// operator added is left alone. Groups emptied by the removal go with them, so
/// a re-run stays byte-stable.
fn retire_legacy_hooks(hooks: &mut Map<String, Value>) {
    let legacy: Vec<String> = ENFORCE_HOOK_SET
        .iter()
        .flat_map(|(_, verb)| superseded_hook_commands(verb))
        .collect();
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
                !command.is_some_and(|command| legacy.iter().any(|old| old == command))
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

/// Render the defect in `repo`'s installed hook set, if it has one.
///
/// The judgment is [`aoa_audit::hook_set_defect`]'s: it belongs beside the plane
/// check, in the crate that owns the enforcement-plane question, rather than in a
/// `pub(crate)` helper of this binary where no library consumer could reach it.
/// What is left here is the rendering both output registers share.
pub(crate) fn enforce_hook_warning(repo: &Path) -> Option<String> {
    hook_set_defect(repo).map(|defect| defect.render_line(repo))
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
    let settings_path = repo.join(SETTINGS_REL);

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
    use std::process::Command;

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

    fn scope(base: &Path, raw: &str) -> WriteScope {
        write_scope(base, raw).expect("supported hook target")
    }

    #[test]
    fn in_repository_shapes_are_in_scope_under_one_normalized_spelling() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let absolute = base.join(".github/workflows/ci.yml");

        for raw in [
            absolute.to_str().unwrap(),
            "./.github/workflows/ci.yml",
            "src/../.github/workflows/ci.yml",
        ] {
            assert_eq!(
                scope(&base, raw),
                WriteScope::Inside(vec![".github/workflows/ci.yml".to_string()]),
                "{raw} resolves back inside the repository"
            );
        }
    }

    /// The bead's subject (aoa-7g14y.1): a target in another directory tree is
    /// not this repository's business, however it is spelled.
    #[test]
    fn targets_outside_the_repository_are_out_of_scope() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let absolute = elsewhere.path().join("notes.md");

        assert_eq!(scope(&base, "../outside.rs"), WriteScope::Outside);
        assert_eq!(
            scope(&base, absolute.to_str().unwrap()),
            WriteScope::Outside
        );
        // A `..` chain walking off the filesystem root cannot be inside either.
        assert_eq!(
            scope(&base, "../../../../../../../../../../etc/passwd"),
            WriteScope::Outside
        );
    }

    /// A repository-local symlink pointing out of the tree stays in scope under
    /// its lexical spelling, so the R5 protected-path match still sees it.
    #[cfg(unix)]
    #[test]
    fn a_repo_local_symlink_leaving_the_tree_stays_in_scope() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), base.join("escape")).unwrap();

        assert_eq!(
            scope(&base, "escape/planted.rs"),
            WriteScope::Inside(vec!["escape/planted.rs".to_string()])
        );
    }

    /// Containment is the only thing that stopped being an error. A target that
    /// names nothing writable is still a failure, and `check` denies on it.
    #[test]
    fn unusable_write_targets_are_still_errors_not_out_of_scope() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();

        assert!(write_scope(&base, "").is_err());
        assert!(write_scope(&base, ".")
            .unwrap_err()
            .to_string()
            .contains("repository root"));
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
        let mut e = event("Write", None);
        e.cwd = outside.path().to_string_lossy().into_owned();

        let err = resolve_base(&e).expect_err("a directory in no repository has no trust root");
        assert!(
            err.to_string().contains("not inside a Git repository"),
            "{err}"
        );
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

    /// The installer and the audit are two sides of one contract, and they have
    /// drifted before: the installer moved to a repo-local wrapper, the plane
    /// check did not know, and a correctly-installed repository audited as
    /// MISSING its runtime hook. Nothing short of running a real install through
    /// the real reader catches that — both sides pass their own tests while
    /// disagreeing about what an install looks like.
    #[test]
    fn a_real_install_satisfies_the_audit_that_reads_it_back() {
        let repo = tempfile::tempdir().unwrap();
        install_enforce_hooks(repo.path()).expect("install into a fresh repo");

        assert_ne!(
            aoa_audit::enforcement_liveness(repo.path(), None),
            aoa_audit::EnforcementLiveness::NotInstalled,
            "the audit must recognise the hook set this installer just wrote; \
             reading a real install as not-installed is the drift this contract exists to stop"
        );
        assert_eq!(
            aoa_audit::hook_set_defect(repo.path()),
            None,
            "a fresh install must be stamped for the current hook set, with a runnable wrapper"
        );
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

    /// Upgrading from any earlier hook set must retire that set's commands, not
    /// sit beside them. Left registered, a superseded command keeps failing the
    /// way its replacement exists to stop — v1's bare `aoa` off PATH, v2's
    /// cwd-relative wrapper from any other directory — and doubles every span in
    /// the repos where it does resolve.
    #[test]
    fn upgrading_retires_every_superseded_hook_set() {
        // Drawn from the retirement list itself, so a shape added there without
        // being retired — or retired without being listed — fails here.
        for index in 0..superseded_hook_commands("").len() {
            let version = index as u64 + 1;
            let command = |verb: &str| superseded_hook_commands(verb)[index].clone();
            let installed = json!({
                "aoa": { "enforce_hook_set_version": version },
                "hooks": {
                    "PostToolUse": [
                        { "matcher": "Bash", "hooks": [{ "type": "command", "command": command("record") }] },
                        { "matcher": mutation_tool_matcher(), "hooks": [{ "type": "command", "command": command("commit") }] },
                    ],
                    "PreToolUse": [
                        { "matcher": mutation_tool_matcher(), "hooks": [{ "type": "command", "command": command("check") }] },
                    ],
                }
            });
            let upgraded =
                merge_enforce_hooks(installed).expect("upgrade from an earlier hook set");

            let rendered = serde_json::to_string(&upgraded).unwrap();
            for (_, verb) in ENFORCE_HOOK_SET {
                assert!(
                    !rendered.contains(&serde_json::to_string(&command(verb)).unwrap()),
                    "no hook-set-{version} command may survive the upgrade: {rendered}"
                );
            }
            assert_eq!(upgraded["aoa"][HOOK_VERSION_KEY], ENFORCE_HOOK_SET_VERSION);
            // Exactly one entry per event/matcher pair: retired, then reinstalled.
            assert_eq!(
                upgraded["hooks"]["PostToolUse"].as_array().unwrap().len(),
                2
            );
            assert_eq!(upgraded["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        }
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
