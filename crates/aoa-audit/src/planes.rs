use std::path::{Path, PathBuf};
use std::process::Command;

use aoa_corpus::PRECOMMIT_HOOK_MARKERS;
use serde_json::Value;

use crate::hook_set::{read_settings, AOA_SETTINGS_KEY, ENFORCE_HOOK_SET, ENFORCE_WRAPPER_REL};
use crate::tier::EnforcementPlane;

const CI_MARKERS: &[&str] = &[
    ".github/workflows",
    ".gitlab-ci.yml",
    ".circleci/config.yml",
];

const AMBIENT_REPOSITORY_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

const MACHINE_CONFIG_ENV: [&str; 2] = ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"];

#[cfg(unix)]
const NULL_DEVICE: &str = "/dev/null";
#[cfg(not(unix))]
const NULL_DEVICE: &str = "NUL";

const DEFAULT_PRE_COMMIT_HOOK: &str = "hooks/pre-commit";

fn present(repo: &Path, plane: EnforcementPlane) -> bool {
    match plane {
        EnforcementPlane::RuntimeHook => runtime_hooks(repo) == RuntimeHooks::Installed,
        EnforcementPlane::PreCommit => {
            any_exists(repo, PRECOMMIT_HOOK_MARKERS) || installed_pre_commit_hook(repo)
        }
        EnforcementPlane::Ci => any_exists(repo, CI_MARKERS),
    }
}

fn any_exists(repo: &Path, markers: &[&str]) -> bool {
    markers.iter().any(|rel| repo.join(rel).exists())
}

fn installed_pre_commit_hook(repo: &Path) -> bool {
    let Ok(repo) = repo.canonicalize() else {
        return false;
    };
    let git_dir = repo.join(".git");
    if !git_dir.exists() {
        return false;
    }
    let location = git_hook_location(&repo).unwrap_or_else(|| HookLocation {
        hook: git_dir.join(DEFAULT_PRE_COMMIT_HOOK),
        common_dir: git_dir.clone(),
        git_dir: git_dir.clone(),
    });
    let Ok(hook) = location.hook.canonicalize() else {
        return false;
    };
    let contained = hook.starts_with(&repo)
        || (location
            .common_dir
            .canonicalize()
            .is_ok_and(|common_dir| hook.starts_with(common_dir))
            && registers_worktree(&location.git_dir, &git_dir));
    contained && std::fs::metadata(&hook).is_ok_and(|meta| meta.is_file() && is_executable(&meta))
}

fn registers_worktree(git_dir: &Path, worktree_git_file: &Path) -> bool {
    let Ok(registered) = std::fs::read_to_string(git_dir.join("gitdir")) else {
        return false;
    };
    match (
        Path::new(registered.trim_end()).canonicalize(),
        worktree_git_file.canonicalize(),
    ) {
        (Ok(registered), Ok(expected)) => registered == expected,
        _ => false,
    }
}

struct HookLocation {
    hook: PathBuf,
    common_dir: PathBuf,
    git_dir: PathBuf,
}

fn git_hook_location(repo: &Path) -> Option<HookLocation> {
    let mut command = Command::new("git");
    for variable in AMBIENT_REPOSITORY_ENV {
        command.env_remove(variable);
    }
    for variable in MACHINE_CONFIG_ENV {
        command.env(variable, NULL_DEVICE);
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(parent) = repo.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    let output = command
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--git-common-dir",
            "--git-dir",
            "--git-path",
            DEFAULT_PRE_COMMIT_HOOK,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let reported = std::str::from_utf8(&output.stdout).ok()?;
    let mut lines = reported.lines();
    let (common_dir, git_dir, hook) = (lines.next()?, lines.next()?, lines.next()?);
    if common_dir.is_empty() || git_dir.is_empty() || hook.is_empty() || lines.next().is_some() {
        return None;
    }
    Some(HookLocation {
        hook: repo.join(hook),
        common_dir: repo.join(common_dir),
        git_dir: repo.join(git_dir),
    })
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeHooks {
    Installed,
    ForeignOnly,
    Missing,
}

pub(crate) fn runtime_hooks(repo: &Path) -> RuntimeHooks {
    let Ok(Some(settings)) = read_settings(repo) else {
        return RuntimeHooks::Missing;
    };
    if ENFORCE_HOOK_SET
        .into_iter()
        .all(|(event, verb)| has_enforce_hook(&settings, event, verb))
    {
        return RuntimeHooks::Installed;
    }
    if carries_aoa_install(&settings) || hook_entries(&settings).next().is_none() {
        return RuntimeHooks::Missing;
    }
    RuntimeHooks::ForeignOnly
}

pub(crate) fn carries_aoa_install(settings: &Value) -> bool {
    settings.get(AOA_SETTINGS_KEY).is_some()
        || hook_entries(settings).filter_map(command).any(|command| {
            ENFORCE_HOOK_SET
                .iter()
                .any(|(_, verb)| is_enforce_command(command, verb))
        })
}

fn hook_entries(settings: &Value) -> impl Iterator<Item = &Value> {
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|events| events.values())
        .flat_map(event_entries)
}

fn event_entries(groups: &Value) -> impl Iterator<Item = &Value> {
    groups
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
}

fn command(hook: &Value) -> Option<&str> {
    hook.get("command").and_then(Value::as_str)
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
        .into_iter()
        .flat_map(event_entries)
        .filter_map(command)
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
    use crate::hook_set::{hook_command, MAX_SETTINGS_BYTES, SETTINGS_REL};

    fn settings(repo: &Path, body: &str) {
        let path = repo.join(SETTINGS_REL);
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
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Installed);

        settings(
            repo.path(),
            r#"{"hooks":{"PostToolUse":[{"hooks":[
                {"command":"aoa enforce record"},
                {"command":"aoa enforce check"}
            ]}]}}"#,
        );
        assert_eq!(
            runtime_hooks(repo.path()),
            RuntimeHooks::Missing,
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
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Installed);
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
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Installed);

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
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Missing);

        settings(
            repo.path(),
            &format!(
                "{{\"padding\":\"{}\"}}",
                "x".repeat(MAX_SETTINGS_BYTES as usize)
            ),
        );
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Missing);
    }

    const FOREIGN_SETTINGS: &str = r#"{"hooks":{
        "PreToolUse":[{"matcher":"Edit|Write","hooks":[
            {"type":"command","command":"\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/guard.sh pre"}
        ]}],
        "SessionStart":[{"hooks":[{"type":"command","command":"echo session"}]}]
    }}"#;

    #[test]
    fn hooks_that_are_all_somebody_elses_are_foreign_and_still_a_missing_plane() {
        let repo = tempfile::tempdir().unwrap();
        settings(repo.path(), FOREIGN_SETTINGS);

        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::ForeignOnly);
        assert!(missing_planes(repo.path()).contains(&EnforcementPlane::RuntimeHook));

        settings(
            repo.path(),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"prompt","prompt":"check the work"}]}]}}"#,
        );
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::ForeignOnly);
    }

    #[test]
    fn a_stamped_install_whose_enforce_hooks_were_removed_is_not_foreign() {
        let repo = tempfile::tempdir().unwrap();
        settings(
            repo.path(),
            r#"{
                "aoa":{"enforce_hook_set_version":3},
                "hooks":{"PreToolUse":[{"hooks":[{"command":"./tools/guard.sh"}]}]}
            }"#,
        );

        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Missing);
    }

    #[test]
    fn a_repo_with_no_hook_command_at_all_is_missing_the_plane() {
        let repo = tempfile::tempdir().unwrap();
        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Missing);

        for body in [
            "{}",
            r#"{"hooks":{}}"#,
            r#"{"hooks":{"PreToolUse":[{"hooks":[]}]}}"#,
        ] {
            settings(repo.path(), body);
            assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Missing, "{body}");
        }
        assert!(missing_planes(repo.path()).contains(&EnforcementPlane::RuntimeHook));
    }

    #[test]
    fn a_partial_enforce_set_beside_other_hooks_is_a_missing_plane_not_a_foreign_one() {
        let repo = tempfile::tempdir().unwrap();
        settings(
            repo.path(),
            r#"{"hooks":{
                "PreToolUse":[{"hooks":[
                    {"command":"aoa enforce check"},
                    {"command":"./tools/guard.sh"}
                ]}]
            }}"#,
        );

        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::Missing);
    }
}
