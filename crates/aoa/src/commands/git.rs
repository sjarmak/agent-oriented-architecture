//! Shared git-subprocess plumbing for the app layer.
//!
//! Several commands shell the same `spawn → check-status → format-stderr`
//! sequence: [`corpus`](super::corpus)'s live revert miner and commit resolver,
//! and [`policy`](super::policy)'s HEAD blob enumeration and blame counting.
//! This module holds the one copy.
//!
//! Two layers, because the four call sites do not all want the same error
//! semantics:
//!
//! * [`spawn`] maps only the spawn failure (git missing / not executable). It
//!   does NOT inspect the exit status — a caller whose non-zero exit is a
//!   *domain signal* (a commit that is simply absent) reads `output.status`
//!   itself and produces its own guidance.
//! * [`checked`] adds the "non-zero exit is a failure, with stderr folded in"
//!   contract and returns the raw stdout bytes, so each caller decodes as it
//!   needs — strict UTF-8 for paths/OIDs (fail loud on garbage), lossy for
//!   blame content that legitimately may not be UTF-8.
//!
//! Both return `Result<_, String>`: the injectable [`aoa_corpus::GitRunner`]
//! contract is `Result<String, String>`, so `corpus` returns the string
//! verbatim; the `anyhow` callers map it at their boundary.

use std::process::{Command, Output};
use std::sync::OnceLock;

use aoa_trace::git_free_of_inherited_state;

#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";

const MACHINE_CONFIG_SCOPES: [&str; 2] = ["system", "global"];

const KEY_NOT_SET_EXIT: i32 = 1;

static OPERATOR_SAFE_DIRECTORIES: OnceLock<Result<Vec<String>, String>> = OnceLock::new();

pub(crate) fn reading_repository_data() -> Result<Command, String> {
    let safe_directories = OPERATOR_SAFE_DIRECTORIES
        .get_or_init(operator_safe_directories)
        .as_ref()
        .map_err(Clone::clone)?;
    let mut command = git_free_of_inherited_state();
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE);
    for directory in safe_directories {
        command.arg("-c").arg(format!("safe.directory={directory}"));
    }
    Ok(command)
}

fn operator_safe_directories() -> Result<Vec<String>, String> {
    let label = "git config --get-all safe.directory";
    let mut command = git_free_of_inherited_state();
    command.args([
        "config",
        "--includes",
        "--null",
        "--show-scope",
        "--get-all",
        "safe.directory",
    ]);
    let output = spawn(command, label)?;
    if output.status.code() == Some(KEY_NOT_SET_EXIT) {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        return Err(format!(
            "`{label}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let listing = String::from_utf8(output.stdout)
        .map_err(|e| format!("`{label}` output was not UTF-8: {e}"))?;
    machine_scoped_values(&listing)
        .ok_or_else(|| format!("`{label}` printed a scope with no value: {listing:?}"))
}

fn machine_scoped_values(listing: &str) -> Option<Vec<String>> {
    let mut fields = listing.split_terminator('\0');
    let mut values = Vec::new();
    while let Some(scope) = fields.next() {
        let value = fields.next()?;
        if MACHINE_CONFIG_SCOPES.contains(&scope) {
            values.push(value.to_string());
        }
    }
    Some(values)
}

/// Run a prepared git `command`, mapping only a spawn failure. The exit status
/// is left for the caller to inspect. `label` names the invocation for the
/// error message.
pub(crate) fn spawn(mut command: Command, label: &str) -> Result<Output, String> {
    command
        .output()
        .map_err(|e| format!("failed to run `{label}`: {e} (is git installed?)"))
}

/// Run a prepared git `command`, returning its raw stdout bytes on success. A
/// spawn failure or a non-zero exit both surface as an error string with stderr
/// folded in; `label` (a human-readable command description carrying the repo /
/// path context) names the invocation. Callers decode the bytes themselves.
pub(crate) fn checked(command: Command, label: &str) -> Result<Vec<u8>, String> {
    let output = spawn(command, label)?;
    if !output.status.success() {
        return Err(format!(
            "`{label}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_returns_stdout_on_success() {
        let mut cmd = git_free_of_inherited_state();
        cmd.args(["--version"]);
        let out = checked(cmd, "git --version").expect("git --version succeeds");
        assert!(
            String::from_utf8_lossy(&out).starts_with("git version"),
            "stdout is the git banner"
        );
    }

    #[test]
    fn checked_folds_stderr_on_nonzero_exit() {
        let mut cmd = git_free_of_inherited_state();
        // A subcommand that always fails with a message on stderr, without
        // needing a repo.
        cmd.args([
            "rev-parse",
            "--resolve-git-dir",
            "/definitely/not/a/git/dir",
        ]);
        let err = checked(cmd, "git rev-parse probe").expect_err("must fail");
        assert!(
            err.starts_with("`git rev-parse probe` failed:"),
            "error carries the label: {err}"
        );
    }

    #[test]
    fn machine_scoped_values_keep_system_and_global_in_the_order_git_read_them() {
        let listing = "system\0/srv/a\0global\0\0local\0/srv/b\0global\0*\0";
        assert_eq!(
            machine_scoped_values(listing),
            Some(vec!["/srv/a".to_string(), String::new(), "*".to_string()])
        );
    }

    #[test]
    fn machine_scoped_values_refuse_a_scope_with_no_value() {
        assert_eq!(machine_scoped_values("system\0/srv/a\0global\0"), None);
        assert_eq!(machine_scoped_values(""), Some(Vec::new()));
    }

    #[test]
    fn spawn_reports_a_missing_binary() {
        let cmd = Command::new("definitely-not-a-real-binary-aoa");
        let err = spawn(cmd, "bogus").expect_err("spawn of a missing binary fails");
        assert!(
            err.contains("failed to run `bogus`") && err.contains("is git installed?"),
            "spawn error names the label and hints at git: {err}"
        );
    }
}
