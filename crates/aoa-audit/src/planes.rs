use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use aoa_corpus::PRECOMMIT_HOOK_MARKERS;
use aoa_trace::{linked_worktree_points_back, RepositoryRootError};
use serde_json::Value;

use crate::error::AuditError;
use crate::hook_set::{
    hook_command, read_settings, superseded_hook_commands, AOA_SETTINGS_KEY, ENFORCE_HOOK_SET,
    ENFORCE_WRAPPER_REL,
};
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

const GIT_DEADLINE: Duration = Duration::from_secs(10);
const GIT_POLL_INTERVAL: Duration = Duration::from_millis(5);

fn present(repo: &Path, plane: EnforcementPlane) -> Result<bool, AuditError> {
    Ok(match plane {
        EnforcementPlane::RuntimeHook => runtime_hooks(repo) == RuntimeHooks::Installed,
        EnforcementPlane::PreCommit => {
            any_exists(repo, PRECOMMIT_HOOK_MARKERS) || installed_pre_commit_hook(repo)?
        }
        EnforcementPlane::Ci => any_exists(repo, CI_MARKERS),
    })
}

fn any_exists(repo: &Path, markers: &[&str]) -> bool {
    markers.iter().any(|rel| repo.join(rel).exists())
}

fn installed_pre_commit_hook(repo: &Path) -> Result<bool, AuditError> {
    let Ok(repo) = repo.canonicalize() else {
        return Ok(false);
    };
    let git_dir = repo.join(".git");
    if !git_dir.exists() {
        return Ok(false);
    }
    let Some(location) = git_hook_location(&repo)? else {
        return Ok(false);
    };
    let Ok(hook) = location.hook.canonicalize() else {
        return Ok(false);
    };
    let own_git_directory = std::fs::symlink_metadata(&git_dir).is_ok_and(|marker| marker.is_dir());
    let common_dir = location.common_dir.canonicalize().ok();
    let serves_checkout = || match &common_dir {
        Some(common_dir) => names_worktree(&location.git_dir, common_dir, &repo),
        None => Ok(false),
    };
    let contained = if hook.starts_with(&repo) {
        own_git_directory || serves_checkout()?
    } else {
        !git_dir.is_symlink()
            && common_dir
                .as_ref()
                .is_some_and(|common_dir| hook.starts_with(common_dir))
            && serves_checkout()?
    };
    Ok(contained
        && std::fs::metadata(&hook).is_ok_and(|meta| meta.is_file() && is_executable(&meta)))
}

fn names_worktree(git_dir: &Path, common_dir: &Path, repo: &Path) -> Result<bool, AuditError> {
    let Ok(resolved) = git_dir.canonicalize() else {
        return Ok(false);
    };
    if resolved != common_dir {
        return Ok(registers_worktree(common_dir, &resolved)
            && linked_worktree_points_back(repo, git_dir).is_ok_and(|points_back| points_back));
    }
    match linked_worktree_points_back(repo, git_dir) {
        Ok(points_back) => Ok(points_back),
        Err(RepositoryRootError::Backlink { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(
                match config_value(repo, git_dir, &["--get", "core.worktree"])? {
                    ConfigValue::Set(worktree) => same_directory(&git_dir.join(worktree), repo),
                    ConfigValue::Unset => names_no_worktree(git_dir, repo)?,
                    ConfigValue::Unreadable => false,
                },
            )
        }
        Err(_) => Ok(false),
    }
}

fn registers_worktree(common_dir: &Path, git_dir: &Path) -> bool {
    let worktrees = common_dir.join("worktrees");
    git_dir.parent() == Some(&worktrees)
        || std::fs::read_dir(&worktrees).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .path()
                    .canonicalize()
                    .is_ok_and(|registered| registered == git_dir)
            })
        })
}

fn same_directory(named: &Path, expected: &Path) -> bool {
    named.canonicalize().is_ok_and(|named| {
        expected
            .canonicalize()
            .is_ok_and(|expected| named == expected)
    })
}

fn names_no_worktree(git_dir: &Path, repo: &Path) -> Result<bool, AuditError> {
    let inside_another_checkout = git_dir.canonicalize().map_or(true, |git_dir| {
        git_dir.file_name().is_some_and(|name| name == ".git")
    });
    if inside_another_checkout || !git_dir.join("config").is_file() {
        return Ok(false);
    }
    Ok(
        match config_value(repo, git_dir, &["--type=bool", "--get", "core.bare"])? {
            ConfigValue::Set(bare) => bare != "true",
            ConfigValue::Unset => true,
            ConfigValue::Unreadable => false,
        },
    )
}

enum ConfigValue {
    Set(String),
    Unset,
    Unreadable,
}

const GIT_CONFIG_KEY_UNSET: i32 = 1;

fn config_value(repo: &Path, git_dir: &Path, query: &[&str]) -> Result<ConfigValue, AuditError> {
    let Some(answer) = answer_within_deadline(
        git(repo)
            .arg("--git-dir")
            .arg(git_dir)
            .arg("config")
            .args(query),
        repo,
    )?
    else {
        return Ok(ConfigValue::Unreadable);
    };
    Ok(
        match (answer.status.code(), std::str::from_utf8(&answer.stdout)) {
            (Some(0), Ok(value)) => {
                ConfigValue::Set(value.strip_suffix('\n').unwrap_or(value).to_string())
            }
            (Some(GIT_CONFIG_KEY_UNSET), _) => ConfigValue::Unset,
            _ => ConfigValue::Unreadable,
        },
    )
}

struct GitAnswer {
    status: ExitStatus,
    stdout: Vec<u8>,
}

fn answer_within_deadline(
    command: &mut Command,
    repo: &Path,
) -> Result<Option<GitAnswer>, AuditError> {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(None);
    };
    let Some(mut pipe) = child.stdout.take() else {
        return Ok(None);
    };
    let (read, stdout_read) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdout = Vec::new();
        let _ = read.send(pipe.read_to_end(&mut stdout).map(|_| stdout));
    });
    let started = Instant::now();
    let unresponsive = || AuditError::GitUnresponsive {
        repo: repo.to_path_buf(),
        seconds: GIT_DEADLINE.as_secs(),
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let remaining = GIT_DEADLINE.saturating_sub(started.elapsed());
                return match stdout_read.recv_timeout(remaining) {
                    Ok(stdout) => Ok(stdout.ok().map(|stdout| GitAnswer { status, stdout })),
                    Err(RecvTimeoutError::Timeout) => Err(unresponsive()),
                    Err(RecvTimeoutError::Disconnected) => Ok(None),
                };
            }
            Ok(None) if started.elapsed() < GIT_DEADLINE => std::thread::sleep(GIT_POLL_INTERVAL),
            waited => {
                let _ = child.kill();
                let _ = child.wait();
                return match waited {
                    Ok(_) => Err(unresponsive()),
                    Err(_) => Ok(None),
                };
            }
        }
    }
}

fn git(repo: &Path) -> Command {
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
    command.arg("-C").arg(repo);
    command
}

struct HookLocation {
    hook: PathBuf,
    common_dir: PathBuf,
    git_dir: PathBuf,
}

fn git_hook_location(repo: &Path) -> Result<Option<HookLocation>, AuditError> {
    let answer = answer_within_deadline(
        git(repo).args([
            "rev-parse",
            "--is-inside-work-tree",
            "--git-common-dir",
            "--git-dir",
            "--git-path",
            DEFAULT_PRE_COMMIT_HOOK,
        ]),
        repo,
    )?;
    Ok(answer.and_then(|answer| reported_hook_location(repo, &answer)))
}

fn reported_hook_location(repo: &Path, answer: &GitAnswer) -> Option<HookLocation> {
    if !answer.status.success() {
        return None;
    }
    let reported = std::str::from_utf8(&answer.stdout).ok()?;
    let mut lines = reported.lines();
    let (inside_work_tree, common_dir, git_dir, hook) =
        (lines.next()?, lines.next()?, lines.next()?, lines.next()?);
    if inside_work_tree != "true" {
        return None;
    }
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
    if hook_set_installed(&settings) {
        return RuntimeHooks::Installed;
    }
    if carries_aoa_install(&settings) || hook_entries(&settings).next().is_none() {
        return RuntimeHooks::Missing;
    }
    RuntimeHooks::ForeignOnly
}

fn hook_set_installed(settings: &Value) -> bool {
    ENFORCE_HOOK_SET
        .into_iter()
        .all(|(event, verb)| has_enforce_hook(settings, event, verb))
}

pub(crate) fn names_enforcement_without_installing_it(settings: &Value) -> bool {
    !hook_set_installed(settings)
        && hook_entries(settings).filter_map(command).any(|command| {
            names_enforcement(command)
                && !ENFORCE_HOOK_SET
                    .iter()
                    .any(|(_, verb)| is_enforce_command(command, verb))
        })
}

fn names_enforcement(command: &str) -> bool {
    let words: Vec<&str> = command
        .split_whitespace()
        .map(|word| word.trim_matches(|c| matches!(c, '"' | '\'' | ';')))
        .collect();
    words.iter().any(|word| word.ends_with(ENFORCE_WRAPPER_REL))
        || words
            .windows(2)
            .any(|pair| pair[0].rsplit('/').next() == Some("aoa") && pair[1] == "enforce")
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

fn has_enforce_hook(settings: &Value, event: &str, verb: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .into_iter()
        .flat_map(event_entries)
        .filter_map(command)
        .any(|command| is_enforce_command(command, verb))
}

fn is_enforce_command(command: &str, verb: &str) -> bool {
    command == hook_command(verb)
        || superseded_hook_commands(verb)
            .iter()
            .any(|superseded| superseded == command)
}

/// Return the enforcement planes that are structurally absent from `repo`, in
/// declaration order. Each absent plane becomes a punch-list item.
pub fn missing_planes(repo: &Path) -> Result<Vec<EnforcementPlane>, AuditError> {
    [
        EnforcementPlane::RuntimeHook,
        EnforcementPlane::PreCommit,
        EnforcementPlane::Ci,
    ]
    .into_iter()
    .filter_map(|plane| match present(repo, plane) {
        Ok(true) => None,
        Ok(false) => Some(Ok(plane)),
        Err(unanswered) => Some(Err(unanswered)),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_set::{MAX_SETTINGS_BYTES, SETTINGS_REL};

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
        assert!(!is_enforce_command(
            "/opt/x.claude/hooks/aoa-enforce record",
            "record"
        ));
    }

    #[test]
    fn a_command_that_only_mentions_the_wrapper_and_verb_does_not_satisfy_the_plane() {
        for decoy in [
            "true # decoy .claude/hooks/aoa-enforce check",
            "echo .claude/hooks/aoa-enforce check",
            "true; : .claude/hooks/aoa-enforce check",
            "check .claude/hooks/aoa-enforce",
            "true || \"${CLAUDE_PROJECT_DIR:-.}\"/.claude/hooks/aoa-enforce check",
            "true # aoa enforce check",
            "/usr/local/bin/aoa enforce check",
        ] {
            assert!(!is_enforce_command(decoy, "check"), "{decoy}");
        }
        for trailing in [
            format!("{} || true", hook_command("check")),
            format!("true # {}", hook_command("check")),
            format!("{} ", hook_command("check")),
        ] {
            assert!(!is_enforce_command(&trailing, "check"), "{trailing}");
        }
    }

    #[test]
    fn every_command_the_installer_has_written_satisfies_the_plane_for_its_own_verb_only() {
        for (_, verb) in ENFORCE_HOOK_SET {
            let written = std::iter::once(hook_command(verb)).chain(superseded_hook_commands(verb));
            for command in written {
                assert!(is_enforce_command(&command, verb), "{command}");
                for (_, other) in ENFORCE_HOOK_SET {
                    assert_eq!(
                        is_enforce_command(&command, other),
                        other == verb,
                        "{command}"
                    );
                }
            }
        }
    }

    fn old_token_matcher(command: &str, verb: &str) -> bool {
        let words: Vec<&str> = command
            .split_whitespace()
            .map(|word| word.trim_matches(|c| matches!(c, '"' | '\'' | ';')))
            .collect();
        if words.iter().any(|word| word.ends_with(ENFORCE_WRAPPER_REL)) {
            return words.contains(&verb);
        }
        let [entrypoint, rest @ ..] = words.as_slice() else {
            return false;
        };
        entrypoint.split('/').next_back() == Some("aoa") && rest == ["enforce", verb]
    }

    fn pick<'a>(index: &mut usize, options: &[&'a str]) -> &'a str {
        let picked = options[*index % options.len()];
        *index /= options.len();
        picked
    }

    #[test]
    fn every_command_the_old_token_matcher_accepted_is_still_accepted_or_warned() {
        let wrapper_lookalike = format!("{ENFORCE_WRAPPER_REL}ment");
        let other_binary = "notaoa enforce";
        let prefixes = [
            "", "true;", "true&&", "true; ", "/opt/x", "./", "\"", "'", "x",
        ];
        let entrypoints = [
            ENFORCE_WRAPPER_REL,
            "aoa enforce",
            "/usr/bin/aoa enforce",
            other_binary,
            &wrapper_lookalike,
        ];
        let joiners = [" ", "  ", "\t"];
        let verbs = ENFORCE_HOOK_SET.map(|(_, verb)| verb);
        let suffixes = ["", ";", "\"", "'", " || true"];
        let combinations = [
            prefixes.len(),
            entrypoints.len(),
            joiners.len(),
            verbs.len(),
            suffixes.len(),
        ]
        .iter()
        .product();

        let mut old_accepted = 0;
        let mut dropped = Vec::new();
        let mut lookalikes_warned = Vec::new();
        for combination in 0..combinations {
            let mut index = combination;
            let prefix = pick(&mut index, &prefixes);
            let entrypoint = pick(&mut index, &entrypoints);
            let joiner = pick(&mut index, &joiners);
            let verb = pick(&mut index, &verbs);
            let suffix = pick(&mut index, &suffixes);
            let command = format!("{prefix}{entrypoint}{joiner}{verb}{suffix}");

            if old_token_matcher(&command, verb) {
                old_accepted += 1;
                if !is_enforce_command(&command, verb) && !names_enforcement(&command) {
                    dropped.push(command.clone());
                }
            }
            if (entrypoint == other_binary || entrypoint == wrapper_lookalike)
                && names_enforcement(&command)
            {
                lookalikes_warned.push(command);
            }
        }

        assert_ne!(old_accepted, 0);
        assert_eq!(dropped, Vec::<String>::new());
        assert_eq!(lookalikes_warned, Vec::<String>::new());
    }

    #[test]
    fn a_decoy_hook_set_under_the_right_events_is_not_an_installed_plane() {
        let repo = tempfile::tempdir().unwrap();
        let mut hooks: serde_json::Map<String, Value> = serde_json::Map::new();
        for (event, verb) in ENFORCE_HOOK_SET {
            hooks
                .entry(event)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"hooks":[{
                    "command": format!("true # decoy {ENFORCE_WRAPPER_REL} {verb}")
                }]}));
        }
        settings(
            repo.path(),
            &serde_json::json!({ "hooks": hooks }).to_string(),
        );

        assert_eq!(runtime_hooks(repo.path()), RuntimeHooks::ForeignOnly);
        assert!(missing_planes(repo.path())
            .unwrap()
            .contains(&EnforcementPlane::RuntimeHook));
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

    fn separate_git_dir_checkout() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let git_dirs = tempfile::tempdir().unwrap();
        let git_dir = git_dirs.path().canonicalize().unwrap().join("module");
        let repo = tempfile::tempdir().unwrap();
        let initialized = git(repo.path())
            .args(["init", "--quiet", "--template=", "--separate-git-dir"])
            .arg(&git_dir)
            .status()
            .unwrap();
        assert!(initialized.success(), "git init failed");
        assert!(names_worktree(&git_dir, &git_dir, repo.path()).unwrap());
        (git_dirs, repo, git_dir)
    }

    #[test]
    fn a_git_dir_whose_config_cannot_be_read_names_no_worktree() {
        let (_git_dirs, repo, git_dir) = separate_git_dir_checkout();
        std::fs::write(git_dir.join("config"), "[core\n").unwrap();

        assert!(matches!(
            config_value(repo.path(), &git_dir, &["--get", "core.worktree"]).unwrap(),
            ConfigValue::Unreadable
        ));
        assert!(!names_worktree(&git_dir, &git_dir, repo.path()).unwrap());
        assert!(!names_no_worktree(&git_dir, repo.path()).unwrap());
    }

    #[test]
    fn a_git_dir_whose_bare_setting_is_not_a_boolean_names_no_worktree() {
        let (_git_dirs, repo, git_dir) = separate_git_dir_checkout();
        std::fs::write(git_dir.join("config"), "[core]\n\tbare = perhaps\n").unwrap();

        assert!(matches!(
            config_value(
                repo.path(),
                &git_dir,
                &["--type=bool", "--get", "core.bare"]
            )
            .unwrap(),
            ConfigValue::Unreadable
        ));
        assert!(!names_no_worktree(&git_dir, repo.path()).unwrap());
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
        assert!(missing_planes(repo.path())
            .unwrap()
            .contains(&EnforcementPlane::RuntimeHook));

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
        assert!(missing_planes(repo.path())
            .unwrap()
            .contains(&EnforcementPlane::RuntimeHook));
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
