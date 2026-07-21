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
//!   path — intent, not outcome.
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
//! The live log is owned by this layer (approach (a)): we control its format, so
//! the gate reads exactly the spans we wrote — no dependency on the host's
//! transcript format. It lands under the same ignored `.aoa/traces/` tree that
//! `observe` already provisions.

use std::fs::{File, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use aoa_codeprobe_shim::bash_runs_tests;
use aoa_enforce::{
    blocked_span, generated_artifact_gate, reproduction_gate, BlockReason, Decision,
};
use aoa_policy::Policy;
use aoa_trace::{Span, SpanSource, SpanType};

use crate::cli::{EnforceArgs, EnforceCommand};
use crate::commands::generated::generated_rules;

/// The tools whose writes the gate guards. A pending call to any of these is a
/// mutation and must be preceded by a reproduction (`test.run`) span.
const MUTATION_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// The Claude Code matcher selecting exactly [`MUTATION_TOOLS`]. Derived from
/// that list rather than spelled out again, so adding a guarded tool cannot
/// leave the hooks matching the old set. Every mutation hook AOA installs
/// shares it.
fn mutation_tool_matcher() -> String {
    MUTATION_TOOLS.join("|")
}

/// How long a hook waits for the span log's lock before failing. Generous
/// against real contention (the lock spans one read and one append) and short
/// enough that a wedged holder cannot stall the session.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll interval while waiting. Short enough to be invisible under normal
/// contention, long enough not to spin a core against a wedged holder.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(5);

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
pub fn run(args: &EnforceArgs) -> Result<i32> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("failed to read hook payload from stdin")?;
    let event: HookEvent = serde_json::from_str(&raw)
        .context("hook payload was not valid JSON with the expected fields")?;

    match args.command {
        EnforceCommand::Record => run_record(&event),
        EnforceCommand::Check => run_check(&event),
        EnforceCommand::Commit => run_outcome(&event, SpanType::WriteCommitted),
        EnforceCommand::Fail => run_outcome(&event, SpanType::WriteFailed),
        EnforceCommand::Deny => run_outcome(&event, SpanType::WriteDenied),
    }
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
    record_write_span(event, span_type)?;
    Ok(0)
}

/// Append one write-lifecycle span carrying the event's target path.
///
/// A mutation call with no resolvable target records nothing: there is no path
/// to hold out, and a pathless write span would be indistinguishable from one
/// whose target was dropped.
fn record_write_span(event: &HookEvent, span_type: SpanType) -> Result<()> {
    if let Some(target) = write_target(event) {
        let log = live_log_path(event)?;
        let mut attributes = Map::new();
        attributes.insert("path".to_string(), Value::String(target.to_string()));
        append_span(&log, span_type, attributes)?;
    }
    Ok(())
}

/// PostToolUse: append a `test.run` span iff the Bash command ran tests. Never
/// blocks.
fn run_record(event: &HookEvent) -> Result<i32> {
    if let Some(span_type) = recorded_span_type(event) {
        let log = live_log_path(event)?;
        append_span(&log, span_type, Map::new())?;
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

    if let (Some(policy), Some(target)) = (&policy, write_target(event)) {
        // R5: protected paths are forbidden outright, regardless of reproduction.
        if policy.compile()?.is_protected(target) {
            return block(event, BlockReason::ProtectedPath(target.to_string()));
        }
        // R6: generated artifacts are derived — redirect the agent to the source
        // rather than letting it hand-edit the artifact.
        if let Decision::Block(reason) = generated_artifact_gate(&generated_rules(policy)?, target)
        {
            return block(event, reason);
        }
    }

    // R7: reproduction gate, on unless the policy explicitly disables it.
    let reproduction_required = policy.as_ref().is_none_or(|p| p.reproduction_required);
    if !reproduction_required {
        return allow(event);
    }

    let log = live_log_path(event)?;
    let prior = read_spans(&log)?;
    match reproduction_gate(&prior) {
        Decision::Allow => allow(event),
        Decision::Block(reason) => block(event, reason),
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
fn allow(event: &HookEvent) -> Result<i32> {
    record_write_span(event, SpanType::WriteAttempt)?;
    Ok(0)
}

/// Emit the `write.blocked` span, surface the reason on stderr, and return the
/// exit code (2) that signals Claude Code to deny the pending tool call.
fn block(event: &HookEvent, reason: BlockReason) -> Result<i32> {
    let log = live_log_path(event)?;
    let message = reason.to_string();
    append_span_with(&log, |seq| blocked_span(seq, reason))?;
    eprintln!("aoa: blocked {} — {message}", event.tool_name);
    Ok(2)
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

/// The repo root the hook fired from: the payload `cwd`, falling back to the
/// process working directory. Both the live log and `aoa-policy.yaml` are rooted
/// here.
fn resolve_base(event: &HookEvent) -> Result<PathBuf> {
    if event.cwd.is_empty() {
        std::env::current_dir().context("failed to resolve current directory")
    } else {
        Ok(PathBuf::from(&event.cwd))
    }
}

/// Resolve the append-only live-log path for this session, under the ignored
/// `.aoa/traces/` tree. The session id is sanitized to a bare filename token so
/// a hostile payload cannot escape the traces directory.
fn live_log_path(event: &HookEvent) -> Result<PathBuf> {
    let session = sanitize_session(&event.session_id);
    Ok(resolve_base(event)?
        .join(".aoa")
        .join("traces")
        .join(format!("live-{session}.jsonl")))
}

/// Reduce a session id to `[A-Za-z0-9_-]`, collapsing everything else. Guarantees
/// the value is a single safe path component (no separators, no `..`). Empty or
/// fully-stripped ids become `unknown` so a log still has a stable home.
fn sanitize_session(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Read the live log into spans, tolerating a missing file (no reproduction yet)
/// but failing loud on a corrupt line — a malformed log is a real defect, not
/// something to silently skip.
///
/// Taken under a bounded **shared** lock, so the read never overlaps an in-flight
/// [`append_span_with`]. Without it a reader could observe a half-written final
/// line, and both ways of handling that are wrong: parsing it fails the hook on
/// input that is merely in flight, while skipping it feeds [`reproduction_gate`]
/// a log missing the very `test.run` being recorded, which blocks the agent's
/// write for a reason that is not true (aoa-wew0).
fn read_spans(log: &Path) -> Result<Vec<Span>> {
    read_spans_within(log, LOCK_TIMEOUT)
}

/// [`read_spans`], with the lock timeout supplied. Exists for the same reason
/// [`append_span_within`] does: a test must exercise the real read path,
/// including that it acquires the lock boundedly, without waiting a full
/// [`LOCK_TIMEOUT`].
fn read_spans_within(log: &Path, lock_timeout: Duration) -> Result<Vec<Span>> {
    let mut file = match File::open(log) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(anyhow!(err)).with_context(|| format!("failed to read {}", log.display()))
        }
    };
    lock_shared_bounded(&file, log, lock_timeout)?;
    read_spans_from(&mut file, log)
}

/// Read an already-open, already-locked log handle into spans, failing loud on
/// any corrupt line.
///
/// Takes the handle rather than the path so the append path can reuse it against
/// the descriptor it already holds the exclusive lock on, rather than opening a
/// second descriptor to re-read the same bytes.
fn read_spans_from(file: &mut File, log: &Path) -> Result<Vec<Span>> {
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .with_context(|| format!("failed to read {}", log.display()))?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Span>(line)
                .with_context(|| format!("corrupt span line in {}", log.display()))
        })
        .collect()
}

/// Append a fresh span of `span_type` with `attributes`, numbered after the
/// spans already present so `seq` stays monotonic.
fn append_span(log: &Path, span_type: SpanType, attributes: Map<String, Value>) -> Result<()> {
    append_span_with(log, |seq| Span {
        span_type,
        source: SpanSource::Native,
        seq,
        attributes,
    })
}

/// Append one span, assigning its `seq` and writing it under an exclusive lock.
///
/// `build` receives the next sequence number, so the number is derived *inside*
/// the locked region. That is the whole point of the closure: deriving `seq`
/// means reading the log and counting it, which together with the append is a
/// read-modify-write. Every mutation now drives two of these (intent at
/// `PreToolUse`, then an outcome), so unsynchronized racers were routine, and
/// they produced duplicate and even *decreasing* `seq` values. A decreasing
/// `seq` is not a cosmetic defect: `validate_trace` rejects the trace,
/// `load_corpus` propagates the error, and `aoa audit` then fails for the whole
/// repo — against an append-only log with no repair path. A caller that built
/// its span before calling would reintroduce exactly that race.
///
/// The lock must be one held per *open file description*, which is what
/// [`File::try_lock`] gives us. Modern OFD locks (`fcntl(F_OFD_SETLK)`) would be
/// equivalent; a classic POSIX record lock (`fcntl(F_SETLK)`) is
/// per-process-per-inode and would not be.
///
/// That per-description property is also why the `seq` count is derived by
/// reading **this** handle rather than reopening the path: a second descriptor
/// would be a separate lock holder, and [`read_spans`] takes a shared lock
/// (aoa-wew0), so counting through it would contend with the exclusive lock this
/// function already holds and die at [`LOCK_TIMEOUT`].
///
/// Readers take a shared lock, so writer-vs-writer and reader-vs-writer are both
/// serialized here; see [`read_spans`] for why an unlocked read was not merely
/// noisy but could produce a wrong gate decision.
fn append_span_with(log: &Path, build: impl FnOnce(u64) -> Span) -> Result<()> {
    append_span_within(log, LOCK_TIMEOUT, build)
}

/// [`append_span_with`], with the lock timeout supplied.
///
/// Exists so a test can exercise the real append path — including that it
/// acquires the lock *boundedly* — without waiting a full [`LOCK_TIMEOUT`].
/// Testing [`lock_exclusive_bounded`] alone would leave the wiring uncovered:
/// swapping this call for a blocking acquisition reintroduces the session-freeze
/// hazard while every test still passes.
fn append_span_within(
    log: &Path,
    lock_timeout: Duration,
    build: impl FnOnce(u64) -> Span,
) -> Result<()> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    // Bound to a named local, never a temporary: dropping the `File` closes the
    // descriptor and releases the lock, so a temporary would unlock immediately
    // and leave the read-modify-write below unguarded.
    //
    // Opened for reading as well as appending so the `seq` count comes from this
    // same locked description — see the note on descriptor identity above.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("failed to open {}", log.display()))?;
    lock_exclusive_bounded(&file, log, lock_timeout)?;

    let span = build(read_spans_from(&mut file, log)?.len() as u64);
    let mut line = serde_json::to_string(&span).context("failed to serialize span")?;
    line.push('\n');
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to append to {}", log.display()))?;
    Ok(())
}

/// Take the log's exclusive lock, giving up after `timeout` rather than waiting
/// forever.
///
/// Hooks run synchronously in the agent's tool path, so a blocking acquisition
/// hands any process that stalls while holding this lock — stopped, or on a
/// wedged network mount — the ability to freeze every later hook in the session
/// with no recovery but killing it by hand. The lock is only ever held across
/// one read and one append, so contention resolves in microseconds; waiting
/// seconds means something is already wrong, and reporting that is strictly
/// better than inheriting the stall.
///
/// Giving up is a real error, never a silent fallback to an unlocked append:
/// that would trade a visible failure for the corrupt `seq` this lock exists to
/// prevent.
fn lock_exclusive_bounded(file: &File, log: &Path, timeout: Duration) -> Result<()> {
    lock_bounded(file, log, timeout, File::try_lock)
}

/// [`lock_exclusive_bounded`] for a reader: takes the log's shared lock, so
/// concurrent readers do not exclude each other but none of them overlaps an
/// in-flight append. Bounded for the same reason the exclusive acquisition is —
/// a reader that inherits a wedged writer's stall freezes the session just as
/// thoroughly as a writer that does.
fn lock_shared_bounded(file: &File, log: &Path, timeout: Duration) -> Result<()> {
    lock_bounded(file, log, timeout, File::try_lock_shared)
}

/// The bounded-acquisition loop both lock modes share; `try_acquire` is the only
/// difference between them.
fn lock_bounded(
    file: &File,
    log: &Path,
    timeout: Duration,
    try_acquire: fn(&File) -> std::result::Result<(), TryLockError>,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match try_acquire(file) {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(anyhow!(
                    "timed out after {timeout:?} waiting for the span log lock on {}; \
                     another aoa hook is holding it and may be wedged",
                    log.display()
                ))
            }
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("failed to lock {}", log.display()))
            }
        }
    }
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
    add_hook(hooks, "PostToolUse", "Bash", "aoa enforce record")?;
    add_hook(hooks, "PreToolUse", &matcher, "aoa enforce check")?;
    // One hook per write outcome, each with its own command string. The distinct
    // commands are still required even though `add_hook` now keys on matcher as
    // well: the host runs every group whose matcher fits, so two entries sharing
    // a command would run it twice per tool call and double every span it emits.
    for (event, command) in [
        ("PostToolUse", "aoa enforce commit"),
        ("PostToolUseFailure", "aoa enforce fail"),
        ("PermissionDenied", "aoa enforce deny"),
    ] {
        add_hook(hooks, event, &matcher, command)?;
    }
    Ok(settings)
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

    Ok(settings_path)
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
    fn sanitize_session_strips_path_traversal() {
        assert_eq!(sanitize_session("../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitize_session("a/b\\c"), "a-b-c");
        assert_eq!(sanitize_session("ok_id-9"), "ok_id-9");
        assert_eq!(sanitize_session("///"), "unknown");
        assert_eq!(sanitize_session(""), "unknown");
    }

    #[test]
    fn live_log_path_stays_inside_traces_dir() {
        let mut e = event("Write", None);
        e.cwd = "/repo".to_string();
        e.session_id = "../escape".to_string();
        let path = live_log_path(&e).unwrap();
        assert_eq!(path, PathBuf::from("/repo/.aoa/traces/live-escape.jsonl"));
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
        assert_eq!(post[0]["hooks"][0]["command"], "aoa enforce record");
        assert_eq!(post[0]["matcher"], "Bash");
        assert_eq!(post[1]["hooks"][0]["command"], "aoa enforce commit");
        assert_eq!(post[1]["matcher"], matcher);

        let pre = &once["hooks"]["PreToolUse"];
        assert_eq!(pre[0]["hooks"][0]["command"], "aoa enforce check");

        for (event, command) in [
            ("PostToolUseFailure", "aoa enforce fail"),
            ("PermissionDenied", "aoa enforce deny"),
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
        let commands: Vec<&str> = ["PostToolUse", "PostToolUseFailure", "PermissionDenied"]
            .iter()
            .filter_map(|event| merged["hooks"][event].as_array())
            .flatten()
            .filter(|g| g["matcher"] == matcher)
            .map(|g| g["hooks"][0]["command"].as_str().unwrap())
            .collect();

        assert_eq!(
            commands,
            ["aoa enforce commit", "aoa enforce fail", "aoa enforce deny"]
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
        for command in ["log-read", "aoa enforce record", "aoa enforce commit"] {
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
                    "hooks": [{ "type": "command", "command": "aoa enforce commit" }],
                }]
            }
        });
        let err = merge_enforce_hooks(seeded).unwrap_err();
        let message = err.to_string();
        for expected in ["aoa enforce commit", "Bash", &mutation_tool_matcher()] {
            assert!(
                message.contains(expected),
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
                    "hooks": [{ "type": "command", "command": "aoa enforce commit" }],
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

    /// A hook must not inherit another process's stall; see
    /// [`lock_exclusive_bounded`] for why an unbounded wait is unsafe here.
    ///
    /// Uses a short timeout rather than [`LOCK_TIMEOUT`] to keep the test fast;
    /// the bounding behaviour is what is under test, not the value.
    #[test]
    fn a_held_lock_fails_the_waiter_instead_of_hanging_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-held.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();

        // A separate open file description, which is what the lock arbitrates on.
        let holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        holder.lock().unwrap();

        let waiter = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        let timeout = Duration::from_millis(50);
        let started = Instant::now();
        let err = lock_exclusive_bounded(&waiter, &log, timeout).unwrap_err();
        let waited = started.elapsed();

        assert!(
            err.to_string().contains("timed out"),
            "must report the timeout, got: {err}"
        );
        assert!(
            waited >= timeout && waited < timeout * 20,
            "must wait roughly the timeout and then give up, waited {waited:?}"
        );

        // Releasing the holder lets the next acquisition through, so the failure
        // is transient contention rather than a permanently poisoned log.
        drop(holder);
        lock_exclusive_bounded(&waiter, &log, timeout).expect("lock is available once released");
    }

    /// The append path itself must acquire the lock boundedly, not merely the
    /// helper that does the acquiring.
    ///
    /// Covering [`lock_exclusive_bounded`] alone leaves the wiring untested:
    /// replacing the call in [`append_span_within`] with a blocking acquisition
    /// reintroduces the session-freeze hazard while every other test still
    /// passes. Verified by exactly that mutation.
    #[test]
    fn a_contended_append_fails_bounded_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-contended.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();

        let holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        holder.lock().unwrap();

        let timeout = Duration::from_millis(50);
        // Run the append off-thread and wait with a deadline, so a regression to
        // a blocking acquisition surfaces as a diagnosable failure here rather
        // than hanging the whole test binary until CI times out.
        let (tx, rx) = std::sync::mpsc::channel();
        let target = log.clone();
        std::thread::spawn(move || {
            let outcome = append_span_within(&target, timeout, |seq| Span {
                span_type: SpanType::WriteCommitted,
                source: SpanSource::Native,
                seq,
                attributes: Map::new(),
            });
            let _ = tx.send(outcome);
        });

        let err = rx
            .recv_timeout(timeout * 20)
            .expect("append never returned — it inherited the holder's stall instead of giving up")
            .expect_err("append must fail while the lock is held");

        assert!(
            err.to_string().contains("timed out"),
            "must report the timeout, got: {err}"
        );
        // Nothing was written: failing to lock must never fall through to an
        // unlocked append, which would reintroduce the corrupt seq. The holder
        // is released first because the reader takes the same lock (aoa-wew0);
        // asserting while it is still held would time out instead of inspecting
        // the file.
        drop(holder);
        assert!(
            read_spans(&log).unwrap().is_empty(),
            "a failed acquisition must not append"
        );
    }

    /// A reader must never observe a half-written final line.
    ///
    /// Claude Code dispatches parallel tool calls, so two hook subprocesses
    /// genuinely run at once and the R7 gate's read races an append. Before
    /// aoa-wew0 that read took no lock: it could parse a torn line, which fails
    /// the hook on input that is merely *in flight*. Tolerating the fragment
    /// instead would be worse — [`reproduction_gate`] blocks when no `test.run`
    /// is present, so dropping the very span being written turns a loud error
    /// into a silent, wrong `ReproductionRequired` block.
    #[test]
    fn a_reader_waits_for_an_in_flight_append_instead_of_tearing_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-inflight.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();

        // An appender caught mid-write: holds the exclusive lock, and only part
        // of its line has reached the file.
        let mut writer = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        writer.lock().unwrap();
        let line = serde_json::to_string(&Span {
            span_type: SpanType::TestRun,
            source: SpanSource::Native,
            seq: 0,
            attributes: Map::new(),
        })
        .unwrap();
        let (head, tail) = line.split_at(line.len() / 2);
        writer.write_all(head.as_bytes()).unwrap();

        let timeout = Duration::from_millis(50);
        let err = read_spans_within(&log, timeout)
            .expect_err("the reader must refuse the torn state, not parse it");
        assert!(
            err.to_string().contains("timed out"),
            "must report waiting for the writer, got: {err}"
        );

        // The appender finishes and releases; the committed state reads cleanly.
        writer.write_all(tail.as_bytes()).unwrap();
        writer.write_all(b"\n").unwrap();
        drop(writer);

        let spans = read_spans_within(&log, timeout).expect("committed state reads cleanly");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].span_type, SpanType::TestRun);
    }

    /// Also the regression guard for the writer deadlocking against itself.
    ///
    /// `read_spans` locks, and [`append_span_within`] derives `seq` by reading
    /// the log inside its own exclusive critical section. `flock` is held per
    /// *open file description*, so deriving that count through a second
    /// descriptor would make every append contend with the lock it already
    /// holds and die at [`LOCK_TIMEOUT`]. The appends below must simply succeed.
    ///
    /// Driven through the bounded variants so that regression fails in
    /// milliseconds rather than after the full production timeout — a guard that
    /// takes 5s per append to report is one people stop running. The unbounded
    /// [`append_span`] and [`read_spans`] wrappers are covered by
    /// `concurrent_appends_assign_distinct_sequence_numbers` and
    /// `read_spans_missing_file_is_empty_not_error`.
    #[test]
    fn append_then_read_round_trips_spans_monotonically() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-x.jsonl");
        let timeout = Duration::from_millis(50);
        for span_type in [SpanType::TestRun, SpanType::WriteAttempt] {
            append_span_within(&log, timeout, |seq| Span {
                span_type,
                source: SpanSource::Native,
                seq,
                attributes: Map::new(),
            })
            .expect("an uncontended append must not wait on itself");
        }
        let spans = read_spans_within(&log, timeout).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].seq, 0);
        assert_eq!(spans[1].seq, 1);
        assert_eq!(spans[0].span_type, SpanType::TestRun);
    }

    #[test]
    fn read_spans_missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let spans = read_spans(&dir.path().join("nope.jsonl")).unwrap();
        assert!(spans.is_empty());
    }

    /// Concurrent appenders must produce one distinct `seq` each.
    ///
    /// Asserting only that the result *validates* would pass vacuously:
    /// `validate_trace` rejects a decreasing `seq` but accepts a repeated one,
    /// so the duplicate half of the race is invisible to it. The real assertion
    /// is that N appends yield exactly the set `0..N`.
    ///
    /// Every racer goes through `append_span`, which opens its own descriptor —
    /// `flock` is held per open file description, so a test sharing one handle
    /// would serialize for the wrong reason and prove nothing.
    #[test]
    fn concurrent_appends_assign_distinct_sequence_numbers() {
        const RACERS: u64 = 20;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-race.jsonl");

        std::thread::scope(|scope| {
            for _ in 0..RACERS {
                scope.spawn(|| append_span(&log, SpanType::WriteCommitted, Map::new()).unwrap());
            }
        });

        let mut seqs: Vec<u64> = read_spans(&log).unwrap().iter().map(|s| s.seq).collect();
        assert_eq!(seqs.len(), RACERS as usize, "every append must land");
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (0..RACERS).collect::<Vec<_>>(),
            "seqs must be distinct and gapless; duplicates or gaps mean the \
             read-modify-write raced"
        );
    }
}
