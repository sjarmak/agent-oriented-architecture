use std::path::Path;

use serde_json::Value;

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

const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Whether Claude settings contain both AOA runtime enforcement hooks.
///
/// Malformed, oversized, non-regular, and incomplete files are treated as a
/// missing plane. The audit reports the resulting Tier-1 finding rather than
/// failing its whole read-only pass on optional host configuration.
fn runtime_hook_present(repo: &Path) -> bool {
    let path = repo.join(".claude/settings.json");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() <= MAX_SETTINGS_BYTES => {}
        _ => return false,
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    let settings: Value = match serde_json::from_str(&raw) {
        Ok(settings) => settings,
        Err(_) => return false,
    };
    [
        ("PostToolUse", "record"),
        ("PreToolUse", "check"),
        ("PostToolUse", "commit"),
        ("PostToolUseFailure", "fail"),
        ("PermissionDenied", "deny"),
    ]
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

fn is_enforce_command(command: &str, verb: &str) -> bool {
    let mut words = command.split_whitespace();
    let Some(entrypoint) = words.next() else {
        return false;
    };
    let rest: Vec<&str> = words.collect();
    // `aoa enforce <verb>` (bare or absolute) and `<path>/aoa-enforce <verb>`
    // are the two shapes the installer has ever written.
    if entrypoint.ends_with("aoa-enforce") {
        return rest.first() == Some(&verb);
    }
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

    fn settings(repo: &Path, body: &str) {
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(repo.join(".claude/settings.json"), body).unwrap();
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

    /// An unrelated command that merely mentions a verb is not the plane. The
    /// match is on AOA's entrypoint plus its verb, not on a substring.
    #[test]
    fn a_lookalike_command_does_not_satisfy_the_plane() {
        assert!(!is_enforce_command("echo aoa enforce record", "record"));
        assert!(!is_enforce_command("aoa-enforcer record", "record"));
        assert!(!is_enforce_command("aoa audit record", "record"));
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
