//! The contract between the installer of AOA's runtime enforcement hooks and
//! the audit that reads them back.
//!
//! One repository, two sides: `aoa observe --enforce` writes hook commands into
//! `.claude/settings.json`, and this crate reads them back to answer whether the
//! runtime plane is present ([`crate::planes`]), current (here), and emitting
//! ([`crate::liveness`]). Those sides used to hold independent copies of the same
//! facts — the wrapper path, the settings path, and the five event/verb pairs
//! were each spelled once in the CLI and once here — and they drifted: the
//! installer moved to a repo-local wrapper, the reader did not know, and a
//! correctly-installed repository audited as MISSING its runtime hook. The reader
//! reported a false negative about the repository's own enforcement plane, which
//! is precisely the failure class AOA ships to detect.
//!
//! So the contract is declared once, here, and the installer composes from it
//! rather than beside it. [`hook_command`] is the constructor both sides go
//! through: a change to the installed spelling cannot reach `settings.json`
//! without also reaching the matcher that reads it back.
//!
//! Declaring it here is also what lets the crate that owns the enforcement-plane
//! question answer more than present/absent. [`hook_set_defect`] reports an
//! install that is behind, ahead, unstamped, or whose wrapper has been deleted —
//! states a boolean "installed" cannot distinguish, and which used to live in a
//! `pub(crate)` helper in the binary where no library consumer could reach them.

use std::path::Path;

use serde_json::Value;

/// The wrapper every installed hook runs, relative to the repository root.
pub const ENFORCE_WRAPPER_REL: &str = ".claude/hooks/aoa-enforce";

/// The host settings file the hook set is installed into, relative to the
/// repository root.
pub const SETTINGS_REL: &str = ".claude/settings.json";

/// Bumped whenever [`hook_command`] changes shape, so a repo carrying the old
/// spelling reports as behind and re-running the installer retires it.
pub const ENFORCE_HOOK_SET_VERSION: u64 = 3;

/// The settings key under which the installer stamps its hook-set version.
pub const AOA_SETTINGS_KEY: &str = "aoa";

/// The stamp itself, inside [`AOA_SETTINGS_KEY`].
pub const HOOK_VERSION_KEY: &str = "enforce_hook_set_version";

/// The exit code Claude Code reads as "deny this tool call"; every other non-zero
/// exit is only a non-blocking warning.
///
/// Part of the hook contract rather than of the enforcement runtime alone:
/// [`hook_command`] embeds it in the command string it installs, so the value has
/// to be readable from the side that generates that string.
pub const BLOCK_EXIT_CODE: i32 = 2;

/// Every host event the enforcement plane occupies, paired with the verb it runs
/// there.
///
/// The installer registers exactly this set and the plane check requires exactly
/// this set. A verb wired under the wrong event is not the plane, and neither is
/// a set missing one of them.
pub const ENFORCE_HOOK_SET: [(&str, &str); 5] = [
    ("PostToolUse", "record"),
    ("PreToolUse", "check"),
    ("PostToolUse", "commit"),
    ("PostToolUseFailure", "fail"),
    ("PermissionDenied", "deny"),
];

/// The command Claude Code runs for one enforce `verb`.
///
/// Hooks run under `/bin/sh` with the host's environment, so the earlier bare
/// `aoa enforce <verb>` form enforced only where the binary happened to be on
/// that environment's PATH. Where it was not, the hooks failed non-blocking and
/// the plane was inert while still reading as installed — the exact
/// silently-degraded guard AOA exists to detect. The command names a repo-local
/// wrapper that resolves the binary itself and is loud when it cannot.
///
/// Finding that wrapper is the one step the wrapper cannot do for itself, so it
/// is guarded here. `CLAUDE_PROJECT_DIR` is the host's repo root; an unset value
/// used to fall back to `.`, which resolved by whatever cwd the host happened to
/// use and exited 127 everywhere else. The host reads 127 as a non-blocking
/// warning, so the write proceeded — the same silently-degraded guard, keyed on
/// cwd instead of PATH. The command now tests the wrapper for executability first
/// and exits with the verb's own unavailable code when it is not there, which
/// also covers a deleted or non-executable wrapper. `"$@"` forwards the
/// operator's extra arguments when the command is run by hand.
#[must_use]
pub fn hook_command(verb: &str) -> String {
    let unavailable = unavailable_exit_code(verb);
    format!(
        "sh -c 'h=\"${{CLAUDE_PROJECT_DIR:-}}\"/{ENFORCE_WRAPPER_REL}; \
         [ -x \"$h\" ] || {{ \
         echo \"aoa-enforce: ENFORCEMENT UNAVAILABLE — no executable hook at $h; \
         is CLAUDE_PROJECT_DIR set? This hook did NOT enforce.\" >&2; \
         exit {unavailable}; }}; \
         exec \"$h\" {verb} \"$@\"' aoa-enforce-hook"
    )
}

/// The exit code a hook uses when enforcement could not run at all.
///
/// `check` is the only blocking hook, so unavailable enforcement must deny rather
/// than fall open. The advisory hooks cannot block; they exit non-zero so the
/// host surfaces the failure instead of recording a span never written. The
/// installed wrapper makes the same split for a missing binary, and the two must
/// agree or the guard's exit code would depend on which step failed.
fn unavailable_exit_code(verb: &str) -> i32 {
    if verb == "check" {
        BLOCK_EXIT_CODE
    } else {
        1
    }
}

/// The largest `.claude/settings.json` any reader in this crate will load.
pub(crate) const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Why `<repo>/.claude/settings.json` could not be believed.
///
/// Distinguished because they read differently to an operator: one says the file
/// could not be loaded at all, the other that it was loaded and is not JSON.
pub(crate) enum SettingsFault {
    Unreadable(String),
    Malformed,
}

/// Load `<repo>/.claude/settings.json`.
///
/// `Ok(None)` means the file is absent, which is a repository with no host
/// settings rather than a fault. `Err` means it exists and cannot be trusted: a
/// symlink, a non-regular file, one larger than [`MAX_SETTINGS_BYTES`], an
/// unreadable one, or one that does not parse.
///
/// Both the plane check and the stamp check read through here, which is the point
/// — they used to apply different rules to the same file, so a symlinked
/// `settings.json` reported the plane MISSING while the stamp beside it reported
/// a healthy install.
pub(crate) fn read_settings(repo: &Path) -> Result<Option<Value>, SettingsFault> {
    let path = repo.join(SETTINGS_REL);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(SettingsFault::Unreadable(
                "not a regular file; a symlinked settings file is not trusted here".to_string(),
            ))
        }
        Ok(metadata) if metadata.len() > MAX_SETTINGS_BYTES => {
            return Err(SettingsFault::Unreadable(format!(
                "larger than the {MAX_SETTINGS_BYTES}-byte limit"
            )))
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(SettingsFault::Unreadable(err.to_string())),
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => return Err(SettingsFault::Unreadable(err.to_string())),
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|_| SettingsFault::Malformed)
}

/// A defect in an installed hook set that [`crate::missing_planes`] cannot see.
///
/// Every variant describes a repository whose runtime plane reads as *present*
/// and is not the plane this binary installs: stamped for another hook set,
/// unstamped, or pointing at a wrapper that is gone. Each one is the
/// installed-but-inert reading the wrapper was introduced to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSetDefect {
    /// The settings file exists and could not be loaded.
    SettingsUnreadable(String),
    /// The settings file is not JSON.
    SettingsMalformed,
    /// No hook-set stamp: installed by a binary that predates stamping, or
    /// hand-edited.
    StampMissing,
    /// The stamp is present and is not a hook-set version.
    StampMalformed,
    /// Stamped for an older hook set, so its commands are the ones the current
    /// set exists to retire.
    Behind { installed: u64 },
    /// Stamped for a newer hook set than this binary knows how to write.
    Ahead { installed: u64 },
    /// The stamp is current and the wrapper its hooks run is absent.
    WrapperMissing,
    /// The wrapper path resolves to something that is not a regular file.
    WrapperNotRegularFile,
    /// The wrapper exists and no execute bit is set, so every hook fails.
    WrapperNotExecutable,
}

impl HookSetDefect {
    /// One line naming the defect and the command that repairs it, for whichever
    /// register the caller renders into.
    #[must_use]
    pub fn render_line(&self, repo: &Path) -> String {
        let settings = repo.join(SETTINGS_REL);
        let settings = settings.display();
        let wrapper = repo.join(ENFORCE_WRAPPER_REL);
        let wrapper = wrapper.display();
        match self {
            HookSetDefect::SettingsUnreadable(reason) => format!(
                "{settings} has an unreadable enforce hook stamp ({reason}); rerun `aoa observe --enforce`"
            ),
            HookSetDefect::SettingsMalformed => format!(
                "{settings} has a malformed enforce hook stamp; repair the JSON and rerun `aoa observe --enforce`"
            ),
            HookSetDefect::StampMissing => format!(
                "{settings} is missing the enforce hook stamp (current version {ENFORCE_HOOK_SET_VERSION}); rerun `aoa observe --enforce`"
            ),
            HookSetDefect::StampMalformed => format!(
                "{settings} has a malformed enforce hook stamp; rerun `aoa observe --enforce`"
            ),
            HookSetDefect::Behind { installed } => format!(
                "{settings} enforce hooks are behind (installed {installed}, current {ENFORCE_HOOK_SET_VERSION}); rerun `aoa observe --enforce`"
            ),
            HookSetDefect::Ahead { installed } => format!(
                "{settings} enforce hooks are ahead of this binary (installed {installed}, current {ENFORCE_HOOK_SET_VERSION}); upgrade AOA before reinstalling"
            ),
            HookSetDefect::WrapperMissing => format!(
                "{wrapper} is missing; the installed enforce hooks have nothing to run — rerun `aoa observe --enforce`"
            ),
            HookSetDefect::WrapperNotRegularFile => format!(
                "{wrapper} is not a regular file; enforce hooks cannot run — rerun `aoa observe --enforce`"
            ),
            HookSetDefect::WrapperNotExecutable => format!(
                "{wrapper} is not executable; enforce hooks cannot run — rerun `aoa observe --enforce`"
            ),
        }
    }
}

/// Report how `repo`'s installed hook set differs from the one this binary
/// writes, or `None` when it matches and its wrapper is runnable.
///
/// A repository with no `.claude/settings.json` has nothing to be stale about and
/// reports `None`; its missing plane is [`crate::missing_planes`]'s finding, not
/// this one.
///
/// Reads only.
#[must_use]
pub fn hook_set_defect(repo: &Path) -> Option<HookSetDefect> {
    let settings = match read_settings(repo) {
        Ok(Some(settings)) => settings,
        Ok(None) => return None,
        Err(SettingsFault::Unreadable(reason)) => {
            return Some(HookSetDefect::SettingsUnreadable(reason))
        }
        Err(SettingsFault::Malformed) => return Some(HookSetDefect::SettingsMalformed),
    };

    let Some(version) = settings
        .get(AOA_SETTINGS_KEY)
        .and_then(Value::as_object)
        .and_then(|aoa| aoa.get(HOOK_VERSION_KEY))
    else {
        return Some(HookSetDefect::StampMissing);
    };
    let Some(installed) = version.as_u64() else {
        return Some(HookSetDefect::StampMalformed);
    };

    match installed.cmp(&ENFORCE_HOOK_SET_VERSION) {
        std::cmp::Ordering::Less => Some(HookSetDefect::Behind { installed }),
        std::cmp::Ordering::Greater => Some(HookSetDefect::Ahead { installed }),
        std::cmp::Ordering::Equal => wrapper_defect(repo),
    }
}

/// Report a current stamp whose wrapper is absent or cannot be run.
///
/// A stamp says which hook set was installed; it cannot say whether the file
/// those hooks run still exists. Without this check a deleted wrapper leaves the
/// settings looking current while every hook fails.
fn wrapper_defect(repo: &Path) -> Option<HookSetDefect> {
    let wrapper = repo.join(ENFORCE_WRAPPER_REL);
    let Ok(metadata) = std::fs::symlink_metadata(&wrapper) else {
        return Some(HookSetDefect::WrapperMissing);
    };
    if !metadata.file_type().is_file() {
        return Some(HookSetDefect::WrapperNotRegularFile);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Some(HookSetDefect::WrapperNotExecutable);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository stamped for `version`, with a runnable wrapper. The stamp
    /// checks care only about the stamp; the hook entries themselves are the
    /// plane check's business and are left out.
    fn stamped_repo(version: u64) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        write_settings(
            repo.path(),
            &serde_json::json!({AOA_SETTINGS_KEY: {HOOK_VERSION_KEY: version}}).to_string(),
        );
        write_wrapper(repo.path(), 0o755);
        repo
    }

    fn write_settings(repo: &Path, body: &str) {
        let path = repo.join(SETTINGS_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn write_wrapper(repo: &Path, _mode: u32) {
        let wrapper = repo.join(ENFORCE_WRAPPER_REL);
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(_mode)).unwrap();
        }
    }

    /// The whole point of the stamp: a repository installed at an earlier hook
    /// set reads as present from every configuration surface while running the
    /// commands the current set exists to retire.
    #[test]
    fn a_stamp_off_the_current_version_is_a_defect_in_both_directions() {
        let behind = stamped_repo(ENFORCE_HOOK_SET_VERSION - 1);
        assert_eq!(
            hook_set_defect(behind.path()),
            Some(HookSetDefect::Behind {
                installed: ENFORCE_HOOK_SET_VERSION - 1
            })
        );

        let ahead = stamped_repo(ENFORCE_HOOK_SET_VERSION + 1);
        assert_eq!(
            hook_set_defect(ahead.path()),
            Some(HookSetDefect::Ahead {
                installed: ENFORCE_HOOK_SET_VERSION + 1
            })
        );

        let current = stamped_repo(ENFORCE_HOOK_SET_VERSION);
        assert_eq!(hook_set_defect(current.path()), None);
    }

    /// An absent settings file is not a stale install. The missing plane is the
    /// plane check's finding; reporting it here too would put two findings on one
    /// fact.
    #[test]
    fn a_repo_with_no_settings_file_has_no_stamp_defect() {
        let repo = tempfile::tempdir().unwrap();
        assert_eq!(hook_set_defect(repo.path()), None);
    }

    #[test]
    fn an_unusable_settings_file_is_reported_rather_than_read_as_absent() {
        let repo = tempfile::tempdir().unwrap();
        write_settings(repo.path(), "{");
        assert_eq!(
            hook_set_defect(repo.path()),
            Some(HookSetDefect::SettingsMalformed)
        );

        write_settings(
            repo.path(),
            &format!(
                "{{\"padding\":\"{}\"}}",
                "x".repeat(MAX_SETTINGS_BYTES as usize)
            ),
        );
        assert!(matches!(
            hook_set_defect(repo.path()),
            Some(HookSetDefect::SettingsUnreadable(_))
        ));
    }

    #[test]
    fn a_settings_file_with_no_usable_stamp_says_which_way_it_is_wrong() {
        let repo = tempfile::tempdir().unwrap();
        write_settings(repo.path(), r#"{"hooks":{}}"#);
        assert_eq!(
            hook_set_defect(repo.path()),
            Some(HookSetDefect::StampMissing)
        );

        write_settings(
            repo.path(),
            &serde_json::json!({AOA_SETTINGS_KEY: {HOOK_VERSION_KEY: "3"}}).to_string(),
        );
        assert_eq!(
            hook_set_defect(repo.path()),
            Some(HookSetDefect::StampMalformed)
        );
    }

    /// A current stamp over a deleted wrapper is the installed-but-inert state
    /// the wrapper was introduced to end: the settings read as healthy and every
    /// hook fails.
    #[test]
    fn a_current_stamp_over_an_unrunnable_wrapper_is_still_a_defect() {
        let repo = stamped_repo(ENFORCE_HOOK_SET_VERSION);
        let wrapper = repo.path().join(ENFORCE_WRAPPER_REL);

        std::fs::remove_file(&wrapper).unwrap();
        assert_eq!(
            hook_set_defect(repo.path()),
            Some(HookSetDefect::WrapperMissing)
        );

        std::fs::create_dir(&wrapper).unwrap();
        assert_eq!(
            hook_set_defect(repo.path()),
            Some(HookSetDefect::WrapperNotRegularFile)
        );
        std::fs::remove_dir(&wrapper).unwrap();

        #[cfg(unix)]
        {
            write_wrapper(repo.path(), 0o644);
            assert_eq!(
                hook_set_defect(repo.path()),
                Some(HookSetDefect::WrapperNotExecutable)
            );
        }
    }

    /// Each defect has to name the file the operator must open, or the message
    /// sends them looking for it.
    #[test]
    fn every_defect_names_the_path_it_is_about() {
        let repo = Path::new("/srv/project");
        for (defect, expected) in [
            (HookSetDefect::StampMissing, SETTINGS_REL),
            (HookSetDefect::SettingsMalformed, SETTINGS_REL),
            (HookSetDefect::Behind { installed: 1 }, SETTINGS_REL),
            (HookSetDefect::WrapperMissing, ENFORCE_WRAPPER_REL),
            (HookSetDefect::WrapperNotExecutable, ENFORCE_WRAPPER_REL),
        ] {
            let line = defect.render_line(repo);
            assert!(
                line.contains(&repo.join(expected).display().to_string()),
                "{defect:?} rendered without its path: {line}"
            );
        }
    }
}
