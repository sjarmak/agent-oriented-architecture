use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};

use aoa_audit::{
    hook_command, hook_set_defect, matchers_running, misplaced_matcher, runs_detached,
    superseded_hook_commands, AOA_SETTINGS_KEY, COMMAND_HOOK_TYPE, ENFORCE_HOOK_SET,
    ENFORCE_HOOK_SET_VERSION, ENFORCE_WRAPPER_REL, HOOK_VERSION_KEY, SETTINGS_REL,
};

const ENFORCE_WRAPPER_SCRIPT: &str = include_str!("../enforce_hook.sh");

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
    for (event, verb, scope) in ENFORCE_HOOK_SET {
        add_hook(hooks, event, &scope.matcher(), &hook_command(verb))?;
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
        .flat_map(|(_, verb, _)| superseded_hook_commands(verb))
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

    if runs_detached(groups, |registered| registered == command) {
        return Err(anyhow!(
            "hook event \"{event}\" runs \"{command}\" with `async` or `asyncRewake` set, \
             so the host does not wait for it and it cannot block a write. Remove that \
             setting from the entry and re-run."
        ));
    }
    let registered = matchers_running(groups, |registered| registered == command);
    if let Some(found) = misplaced_matcher(&registered, matcher) {
        return Err(anyhow!(
            "hook event \"{event}\" already runs \"{command}\" under {}, but it \
             must run under matcher \"{matcher}\". Remove or correct that entry \
             and re-run.",
            found.map_or_else(
                || "a group with no matcher".to_string(),
                |m| format!("matcher \"{m}\"")
            )
        ));
    }
    if registered.is_empty() {
        groups.push(json!({
            "matcher": matcher,
            "hooks": [{ "type": COMMAND_HOOK_TYPE, "command": command }],
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoa_audit::HookScope;

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
        let matcher = HookScope::Mutation.matcher();
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
        assert_ne!(
            aoa_audit::enforcement_liveness(repo.path(), None),
            aoa_audit::EnforcementLiveness::ForeignHooks,
            "a real install must not read as somebody else's hook set"
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
        let matcher = HookScope::Mutation.matcher();
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
                        { "matcher": HookScope::Mutation.matcher(), "hooks": [{ "type": "command", "command": command("commit") }] },
                    ],
                    "PreToolUse": [
                        { "matcher": HookScope::Mutation.matcher(), "hooks": [{ "type": "command", "command": command("check") }] },
                    ],
                }
            });
            let upgraded =
                merge_enforce_hooks(installed).expect("upgrade from an earlier hook set");

            let rendered = serde_json::to_string(&upgraded).unwrap();
            for (_, verb, _) in ENFORCE_HOOK_SET {
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
            HookScope::Mutation.matcher(),
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
    fn the_installer_and_the_audit_both_refuse_an_enforce_hook_the_host_does_not_wait_for() {
        for (key, alone) in [
            ("async", true),
            ("async", false),
            ("asyncRewake", true),
            ("asyncRewake", false),
        ] {
            let repo = tempfile::tempdir().unwrap();
            let mut installed = merge_enforce_hooks(json!({})).unwrap();
            let groups = installed["hooks"]["PreToolUse"].as_array_mut().unwrap();
            if alone {
                groups[0]["hooks"][0][key] = true.into();
            } else {
                groups.push(json!({
                    "matcher": HookScope::Mutation.matcher(),
                    "hooks": [{ "type": "command", "command": hook_command("check"), key: true }],
                }));
            }
            let settings = repo.path().join(SETTINGS_REL);
            std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
            std::fs::write(&settings, installed.to_string()).unwrap();
            let wrapper = repo.path().join(ENFORCE_WRAPPER_REL);
            std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
            std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            }

            let refused = merge_enforce_hooks(installed).unwrap_err().to_string();
            for expected in ["PreToolUse", &hook_command("check"), "cannot block a write"] {
                assert!(
                    refused.contains(expected),
                    "{key}, alone: {alone}, {expected:?} missing from: {refused}"
                );
            }
            assert_eq!(
                hook_set_defect(repo.path()),
                Some(aoa_audit::HookSetDefect::HookDetached {
                    event: "PreToolUse",
                    verb: "check",
                }),
                "{key}, alone: {alone}"
            );
        }
    }

    #[test]
    fn the_installer_leaves_a_hook_that_says_it_is_not_detached_as_it_stands() {
        let mut installed = merge_enforce_hooks(json!({})).unwrap();
        installed["hooks"]["PreToolUse"][0]["hooks"][0]["async"] = false.into();

        assert_eq!(merge_enforce_hooks(installed.clone()).unwrap(), installed);
    }

    #[test]
    fn the_installer_and_the_audit_agree_on_a_hook_that_also_runs_outside_its_scope() {
        let misplaced = json!({
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": hook_command("commit") }],
        });
        for at_the_front in [true, false] {
            let repo = tempfile::tempdir().unwrap();
            let mut installed = merge_enforce_hooks(json!({})).unwrap();
            let groups = installed["hooks"]["PostToolUse"].as_array_mut().unwrap();
            let at = if at_the_front { 0 } else { groups.len() };
            groups.insert(at, misplaced.clone());
            let settings = repo.path().join(SETTINGS_REL);
            std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
            std::fs::write(&settings, installed.to_string()).unwrap();
            let wrapper = repo.path().join(ENFORCE_WRAPPER_REL);
            std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
            std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            }

            let refused = merge_enforce_hooks(installed).unwrap_err().to_string();
            assert!(
                refused.contains("under matcher \"Bash\""),
                "misplaced group first: {at_the_front}, got: {refused}"
            );
            assert!(
                matches!(
                    hook_set_defect(repo.path()),
                    Some(aoa_audit::HookSetDefect::HookScopeMismatch {
                        verb: "commit",
                        found: Some(ref found),
                        ..
                    }) if found == "Bash"
                ),
                "misplaced group first: {at_the_front}, got: {:?}",
                hook_set_defect(repo.path())
            );
        }
    }
}
