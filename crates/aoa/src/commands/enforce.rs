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
//!   signal that blocks the tool call), surfacing the reason on stderr.
//!
//! The live log is owned by this layer (approach (a)): we control its format, so
//! the gate reads exactly the spans we wrote — no dependency on the host's
//! transcript format. It lands under the same ignored `.aoa/traces/` tree that
//! `observe` already provisions.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use aoa_codeprobe_shim::bash_runs_tests;
use aoa_enforce::{blocked_span, reproduction_gate, BlockReason, Decision};
use aoa_policy::Policy;
use aoa_trace::{Span, SpanSource, SpanType};

use crate::cli::{EnforceArgs, EnforceCommand};

/// The tools whose writes the gate guards. A pending call to any of these is a
/// mutation and must be preceded by a reproduction (`test.run`) span.
const MUTATION_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

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
    }
}

/// PostToolUse: append a `test.run` span iff the Bash command ran tests. Never
/// blocks.
fn run_record(event: &HookEvent) -> Result<i32> {
    if let Some(span_type) = recorded_span_type(event) {
        let log = live_log_path(event)?;
        append_span(&log, span_type)?;
    }
    Ok(0)
}

/// PreToolUse: block the pending write when it targets a policy-protected path
/// (R5) or when no reproduction precedes it (R7). Protected-path takes
/// precedence — it is unconditional, while the reproduction gate is skippable by
/// policy.
fn run_check(event: &HookEvent) -> Result<i32> {
    if !MUTATION_TOOLS.contains(&event.tool_name.as_str()) {
        // Not a guarded mutation; nothing to gate.
        return Ok(0);
    }

    let base = resolve_base(event)?;
    let policy = load_policy(&base)?;

    // R5: protected paths are forbidden outright, regardless of reproduction.
    if let (Some(policy), Some(target)) = (&policy, write_target(event)) {
        if policy.compile()?.is_protected(target) {
            return block(event, BlockReason::ProtectedPath(target.to_string()));
        }
    }

    // R7: reproduction gate, on unless the policy explicitly disables it.
    let reproduction_required = policy.as_ref().is_none_or(|p| p.reproduction_required);
    if !reproduction_required {
        return Ok(0);
    }

    let log = live_log_path(event)?;
    let prior = read_spans(&log)?;
    match reproduction_gate(&prior) {
        Decision::Allow => Ok(0),
        Decision::Block(reason) => block(event, reason),
    }
}

/// Emit the `write.blocked` span, surface the reason on stderr, and return the
/// exit code (2) that signals Claude Code to deny the pending tool call.
fn block(event: &HookEvent, reason: BlockReason) -> Result<i32> {
    let log = live_log_path(event)?;
    let next_seq = read_spans(&log)?.len() as u64;
    let message = reason.to_string();
    append_span_value(&log, blocked_span(next_seq, reason))?;
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
fn read_spans(log: &Path) -> Result<Vec<Span>> {
    let raw = match std::fs::read_to_string(log) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(anyhow!(err)).with_context(|| format!("failed to read {}", log.display()))
        }
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Span>(line)
                .with_context(|| format!("corrupt span line in {}", log.display()))
        })
        .collect()
}

/// Append a fresh span of `span_type`, numbered after the spans already present
/// so `seq` stays monotonic.
fn append_span(log: &Path, span_type: SpanType) -> Result<()> {
    let next_seq = read_spans(log)?.len() as u64;
    let span = Span {
        span_type,
        source: SpanSource::Native,
        seq: next_seq,
        attributes: Map::new(),
    };
    append_span_value(log, span)
}

/// Serialize one span as a JSONL line and append it, creating the traces
/// directory on first write.
fn append_span_value(log: &Path, span: Span) -> Result<()> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(&span).context("failed to serialize span")?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("failed to open {}", log.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to append to {}", log.display()))?;
    Ok(())
}

/// Merge the two enforcement hook entries into an existing `.claude/settings.json`
/// value, idempotently. Re-running produces a byte-identical result: an entry is
/// added only when no hook with the same command string is already registered
/// under its event. Pure so `observe --enforce` can test the merge in isolation.
pub(crate) fn merge_enforce_hooks(mut settings: Value) -> Value {
    if !settings.is_object() {
        settings = json!({});
    }
    let hooks = settings
        .as_object_mut()
        .expect("settings is an object")
        .entry("hooks")
        .or_insert_with(|| json!({}));

    add_hook(hooks, "PostToolUse", "Bash", "aoa enforce record");
    add_hook(
        hooks,
        "PreToolUse",
        "Write|Edit|MultiEdit|NotebookEdit",
        "aoa enforce check",
    );
    settings
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

/// Ensure `hooks[event]` contains a matcher group running `command`. Idempotent:
/// a no-op if an entry with that command already exists anywhere under `event`.
fn add_hook(hooks: &mut Value, event: &str, matcher: &str, command: &str) {
    let groups = hooks
        .as_object_mut()
        .expect("hooks is an object")
        .entry(event)
        .or_insert_with(|| json!([]));
    let Some(groups) = groups.as_array_mut() else {
        return;
    };

    let already_present = groups.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|inner| {
                inner
                    .iter()
                    .any(|h| h.get("command").and_then(Value::as_str) == Some(command))
            })
    });
    if already_present {
        return;
    }

    groups.push(json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": command }],
    }));
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

    #[test]
    fn merge_enforce_hooks_is_idempotent() {
        let once = merge_enforce_hooks(json!({}));
        let twice = merge_enforce_hooks(once.clone());
        assert_eq!(once, twice, "second merge must be a no-op");

        let post = &once["hooks"]["PostToolUse"];
        assert_eq!(post.as_array().unwrap().len(), 1);
        assert_eq!(post[0]["hooks"][0]["command"], "aoa enforce record");
        let pre = &once["hooks"]["PreToolUse"];
        assert_eq!(pre[0]["hooks"][0]["command"], "aoa enforce check");
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
        let merged = merge_enforce_hooks(existing);
        assert_eq!(merged["model"], "claude-opus-4-8");
        // Existing Read hook retained, our Bash hook added alongside it.
        let post = merged["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert!(post.iter().any(|g| g["hooks"][0]["command"] == "log-read"));
        assert!(post
            .iter()
            .any(|g| g["hooks"][0]["command"] == "aoa enforce record"));
    }

    #[test]
    fn append_then_read_round_trips_spans_monotonically() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-x.jsonl");
        append_span(&log, SpanType::TestRun).unwrap();
        append_span(&log, SpanType::WriteAttempt).unwrap();
        let spans = read_spans(&log).unwrap();
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
}
