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
        let mut cmd = Command::new("git");
        cmd.args(["--version"]);
        let out = checked(cmd, "git --version").expect("git --version succeeds");
        assert!(
            String::from_utf8_lossy(&out).starts_with("git version"),
            "stdout is the git banner"
        );
    }

    #[test]
    fn checked_folds_stderr_on_nonzero_exit() {
        let mut cmd = Command::new("git");
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
    fn spawn_reports_a_missing_binary() {
        let cmd = Command::new("definitely-not-a-real-binary-aoa");
        let err = spawn(cmd, "bogus").expect_err("spawn of a missing binary fails");
        assert!(
            err.contains("failed to run `bogus`") && err.contains("is git installed?"),
            "spawn error names the label and hints at git: {err}"
        );
    }
}
