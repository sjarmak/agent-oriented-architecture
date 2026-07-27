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
        ("PostToolUse", "aoa enforce record"),
        ("PreToolUse", "aoa enforce check"),
        ("PostToolUse", "aoa enforce commit"),
        ("PostToolUseFailure", "aoa enforce fail"),
        ("PermissionDenied", "aoa enforce deny"),
    ]
    .into_iter()
    .all(|(event, command)| has_hook_command(&settings, event, command))
}

fn has_hook_command(settings: &Value, event: &str, expected: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .any(|hook| hook.get("command").and_then(Value::as_str) == Some(expected))
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
