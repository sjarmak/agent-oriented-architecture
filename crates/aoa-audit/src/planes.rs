use std::path::Path;

use serde_json::Value;

use crate::hook_set::{read_settings, ENFORCE_HOOK_SET, ENFORCE_WRAPPER_REL};
use crate::tier::EnforcementPlane;

/// The candidate paths probed for each enforcement plane. A plane is present if
/// any of its candidates exists; absent otherwise. Runtime hooks are the
/// exception: [`runtime_hook_present`] validates the load-bearing hook set
/// rather than trusting a settings filename alone.
fn candidates(plane: EnforcementPlane) -> &'static [&'static str] {
    match plane {
        EnforcementPlane::RuntimeHook => &[],
        EnforcementPlane::PreCommit => &[".pre-commit-config.yaml", ".git/hooks/pre-commit"],
        EnforcementPlane::Ci => &[
            ".github/workflows",
            ".gitlab-ci.yml",
            ".circleci/config.yml",
        ],
    }
}

/// Whether `plane` is structurally present in `repo`.
fn present(repo: &Path, plane: EnforcementPlane) -> bool {
    match plane {
        EnforcementPlane::RuntimeHook => runtime_hook_present(repo),
        _ => candidates(plane).iter().any(|rel| repo.join(rel).exists()),
    }
}

/// Whether Claude settings contain every AOA runtime enforcement hook.
///
/// Malformed, oversized, non-regular, and incomplete files are treated as a
/// missing plane. The audit reports the resulting Tier-1 finding rather than
/// failing its whole read-only pass on optional host configuration.
///
/// The event/verb set is [`ENFORCE_HOOK_SET`], the same declaration the installer
/// writes from, so a hook added or moved on the writing side cannot leave this
/// check looking for the previous shape.
///
/// Shared with [`crate::liveness`], which asks the follow-up question this one
/// cannot answer: an installed hook set is not a running one. [`crate::hook_set`]
/// asks the other one: an installed hook set is not necessarily *this binary's*.
pub(crate) fn runtime_hook_present(repo: &Path) -> bool {
    let Ok(Some(settings)) = read_settings(repo) else {
        return false;
    };
    ENFORCE_HOOK_SET
        .into_iter()
        .all(|(event, verb)| has_enforce_hook(&settings, event, verb))
}

/// Does `event` carry a hook running AOA's enforcement for `verb`?
///
/// Matched by entrypoint and verb rather than by one exact string: the
/// installer moved from a bare `aoa enforce <verb>` to a repo-local
/// `.claude/hooks/aoa-enforce <verb>` wrapper, and a repository may legitimately
/// still be on either. What the audit cares about is that the verb is wired to
/// AOA's enforcement entrypoint under the right event, not how the operator
/// spells the path to it.
fn has_enforce_hook(settings: &Value, event: &str, verb: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|hook| hook.get("command").and_then(Value::as_str))
        .any(|command| is_enforce_command(command, verb))
}

/// Whether `command` runs AOA's enforcement entrypoint for `verb`.
///
/// The wrapper path comes from [`ENFORCE_WRAPPER_REL`] — the same constant the
/// installer writes into every command — and the whole path is matched, never an
/// `aoa-enforce` suffix. A command that merely ends in those characters
/// (`xaoa-enforce record`, `./tools/my-aoa-enforce record`) belongs to somebody
/// else, and reporting the runtime plane present because of it is the exact
/// failure class AOA ships to detect.
fn is_enforce_command(command: &str, verb: &str) -> bool {
    // Shell punctuation around a word is quoting, not part of it: the installed
    // command wraps its script in single quotes and its paths in double quotes.
    let words: Vec<&str> = command
        .split_whitespace()
        .map(|word| word.trim_matches(|c| matches!(c, '"' | '\'' | ';')))
        .collect();

    // Wrapper form: some shell shape runs the installer's own wrapper for this
    // verb. Matching the path and the verb rather than the whole command line
    // keeps the audit reading installations written by an older `aoa` — the v2
    // `sh -c` guard and the plain `<root>/.claude/hooks/aoa-enforce <verb>` it
    // replaced both satisfy it.
    if words.iter().any(|word| word.ends_with(ENFORCE_WRAPPER_REL)) {
        return words.contains(&verb);
    }

    // v1: `aoa enforce <verb>`, bare or by absolute path. Retired by the
    // installer on upgrade, but a repo that has not re-run it still enforces.
    let [entrypoint, rest @ ..] = words.as_slice() else {
        return false;
    };
    entrypoint.split('/').next_back() == Some("aoa") && rest == ["enforce", verb]
}

/// Return the enforcement planes that are structurally absent from `repo`, in
/// declaration order. Each absent plane becomes a punch-list item.
pub fn missing_planes(repo: &Path) -> Vec<EnforcementPlane> {
    [
        EnforcementPlane::RuntimeHook,
        EnforcementPlane::PreCommit,
        EnforcementPlane::Ci,
    ]
    .into_iter()
    .filter(|plane| !present(repo, *plane))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_set::{hook_command, MAX_SETTINGS_BYTES};

    fn settings(repo: &Path, body: &str) {
        let path = repo.join(crate::hook_set::SETTINGS_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn runtime_plane_requires_the_current_hook_set_under_its_events() {
        let repo = tempfile::tempdir().unwrap();
        settings(
            repo.path(),
            r#"{"hooks":{
                "PostToolUse":[{"hooks":[
                    {"command":"aoa enforce record"},
                    {"command":"aoa enforce commit"}
                ]}],
                "PreToolUse":[{"hooks":[{"command":"aoa enforce check"}]}],
                "PostToolUseFailure":[{"hooks":[{"command":"aoa enforce fail"}]}],
                "PermissionDenied":[{"hooks":[{"command":"aoa enforce deny"}]}]
            }}"#,
        );
        assert!(runtime_hook_present(repo.path()));

        settings(
            repo.path(),
            r#"{"hooks":{"PostToolUse":[{"hooks":[
                {"command":"aoa enforce record"},
                {"command":"aoa enforce check"}
            ]}]}}"#,
        );
        assert!(
            !runtime_hook_present(repo.path()),
            "a command under the wrong event cannot forge the plane"
        );
    }

    /// The installer now writes a repo-local wrapper rather than a bare `aoa`,
    /// because the bare form only ran where the binary happened to be on the
    /// host's PATH. The plane check has to recognise that shape, or every
    /// correctly-installed repo audits as missing its runtime hook.
    #[test]
    fn runtime_plane_accepts_the_repo_local_wrapper_form() {
        let repo = tempfile::tempdir().unwrap();
        settings(
            repo.path(),
            r#"{"hooks":{
                "PostToolUse":[{"hooks":[
                    {"command":"\"${CLAUDE_PROJECT_DIR:-.}\"/.claude/hooks/aoa-enforce record"},
                    {"command":"\"${CLAUDE_PROJECT_DIR:-.}\"/.claude/hooks/aoa-enforce commit"}
                ]}],
                "PreToolUse":[{"hooks":[{"command":"\"${CLAUDE_PROJECT_DIR:-.}\"/.claude/hooks/aoa-enforce check"}]}],
                "PostToolUseFailure":[{"hooks":[{"command":"\"${CLAUDE_PROJECT_DIR:-.}\"/.claude/hooks/aoa-enforce fail"}]}],
                "PermissionDenied":[{"hooks":[{"command":"\"${CLAUDE_PROJECT_DIR:-.}\"/.claude/hooks/aoa-enforce deny"}]}]
            }}"#,
        );
        assert!(runtime_hook_present(repo.path()));
    }

    /// The reader must accept the command the writer actually composes, built
    /// here through the same constructor rather than transcribed. A transcribed
    /// fixture is a third copy of the contract and drifts like the second one
    /// did: the shape this test used to hand-spell had already diverged from
    /// [`hook_command`] in both its message and its exit code, so it would have
    /// gone on passing against a command no installer writes.
    #[test]
    fn the_plane_check_accepts_the_command_the_installer_composes() {
        let repo = tempfile::tempdir().unwrap();
        let mut hooks: serde_json::Map<String, Value> = serde_json::Map::new();
        for (event, verb) in ENFORCE_HOOK_SET {
            hooks
                .entry(event)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"hooks":[{"command": hook_command(verb)}]}));
        }
        settings(
            repo.path(),
            &serde_json::json!({ "hooks": hooks }).to_string(),
        );
        assert!(runtime_hook_present(repo.path()));

        // One verb's command must not satisfy another's, or a settings file
        // holding five copies of the same hook would read as the full set.
        assert!(!has_enforce_hook(
            &serde_json::json!({"hooks":{"PreToolUse":[{"hooks":[
                {"command": hook_command("record")}
            ]}]}}),
            "PreToolUse",
            "check"
        ));
    }

    /// An unrelated command that merely mentions a verb is not the plane. The
    /// match is on AOA's own wrapper path (or its v1 entrypoint) plus its verb,
    /// never on a suffix: `aoa-enforce` as a *substring* of another program's
    /// name made someone else's hook report AOA's plane as installed, which is
    /// the failure class this crate exists to detect.
    #[test]
    fn a_lookalike_command_does_not_satisfy_the_plane() {
        assert!(!is_enforce_command("echo aoa enforce record", "record"));
        assert!(!is_enforce_command("aoa-enforcer record", "record"));
        assert!(!is_enforce_command("aoa audit record", "record"));
        assert!(!is_enforce_command("xaoa-enforce record", "record"));
        assert!(!is_enforce_command(
            "/opt/evil/notaoa-enforce record",
            "record"
        ));
        assert!(!is_enforce_command(
            "./tools/my-aoa-enforce record",
            "record"
        ));
        assert!(is_enforce_command(
            "/usr/local/bin/aoa enforce record",
            "record"
        ));
    }

    #[test]
    fn malformed_or_oversized_settings_do_not_satisfy_the_plane() {
        let repo = tempfile::tempdir().unwrap();
        settings(repo.path(), "{");
        assert!(!runtime_hook_present(repo.path()));

        settings(
            repo.path(),
            &format!(
                "{{\"padding\":\"{}\"}}",
                "x".repeat(MAX_SETTINGS_BYTES as usize)
            ),
        );
        assert!(!runtime_hook_present(repo.path()));
    }
}
