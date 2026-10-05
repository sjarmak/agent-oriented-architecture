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

use std::io::Write;
use std::process::{Command, Output};
use std::sync::OnceLock;

use aoa_trace::git_free_of_inherited_state;
use tempfile::NamedTempFile;

const MACHINE_CONFIG_SCOPES: [&[u8]; 2] = [b"system", b"global"];

const KEY_NOT_SET_EXIT: i32 = 1;

static OPERATOR_TRUST_CONFIG: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();

pub(crate) struct RepositoryDataGit {
    pub(crate) command: Command,
    operator_trust: NamedTempFile,
}

impl RepositoryDataGit {
    pub(crate) fn spawn(self, label: &str) -> Result<Output, String> {
        self.run(label, spawn)
    }

    pub(crate) fn checked(self, label: &str) -> Result<Vec<u8>, String> {
        self.run(label, checked)
    }

    fn run<T>(self, label: &str, run: fn(Command, &str) -> Result<T, String>) -> Result<T, String> {
        let outcome = run(self.command, label);
        self.operator_trust
            .close()
            .map_err(|e| format!("failed to remove the git trust config for `{label}`: {e}"))?;
        outcome
    }
}

pub(crate) fn reading_repository_data() -> Result<RepositoryDataGit, String> {
    let trust_config = OPERATOR_TRUST_CONFIG
        .get_or_init(operator_trust_config)
        .as_ref()
        .map_err(Clone::clone)?;
    let mut operator_trust = tempfile::Builder::new()
        .prefix("aoa-git-trust-")
        .tempfile()
        .map_err(|e| format!("failed to create the git trust config: {e}"))?;
    operator_trust
        .write_all(trust_config)
        .and_then(|()| operator_trust.flush())
        .map_err(|e| format!("failed to write the git trust config: {e}"))?;
    let mut command = git_free_of_inherited_state();
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", operator_trust.path());
    Ok(RepositoryDataGit {
        command,
        operator_trust,
    })
}

fn operator_trust_config() -> Result<Vec<u8>, String> {
    let label = "git config --get-all safe.directory";
    let removed_on_drop = tempfile::tempdir()
        .map_err(|e| format!("failed to create an empty directory for `{label}`: {e}"))?;
    let empty_directory = removed_on_drop
        .path()
        .canonicalize()
        .map_err(|e| format!("failed to resolve the empty directory for `{label}`: {e}"))?;
    let ceiling = empty_directory
        .parent()
        .ok_or_else(|| format!("the empty directory for `{label}` has no parent"))?;
    let ceilings = std::env::join_paths([ceiling])
        .map_err(|e| format!("cannot stop `{label}` discovering a repository: {e}"))?;
    let mut command = git_free_of_inherited_state();
    command
        .current_dir(&empty_directory)
        .env("GIT_CEILING_DIRECTORIES", ceilings)
        .args([
            "config",
            "--includes",
            "--null",
            "--show-scope",
            "--get-all",
            "safe.directory",
        ]);
    let output = spawn(command, label)?;
    if output.status.code() == Some(KEY_NOT_SET_EXIT) {
        return Ok(trust_config(&[]));
    }
    let listing = stdout_of_success(output, label)?;
    let values = machine_scoped_values(&listing).ok_or_else(|| {
        format!(
            "`{label}` printed a scope with no value: {:?}",
            String::from_utf8_lossy(&listing)
        )
    })?;
    Ok(trust_config(&values))
}

fn machine_scoped_values(listing: &[u8]) -> Option<Vec<&[u8]>> {
    let mut fields = null_terminated_fields(listing);
    let mut values = Vec::new();
    while let Some(scope) = fields.next() {
        let value = fields.next()?;
        if MACHINE_CONFIG_SCOPES.contains(&scope) {
            values.push(value);
        }
    }
    Some(values)
}

fn null_terminated_fields(listing: &[u8]) -> impl Iterator<Item = &[u8]> {
    let fields = listing.split(|byte| *byte == 0);
    let terminators = listing.iter().filter(|byte| **byte == 0).count();
    fields.take(terminators)
}

fn trust_config(safe_directories: &[&[u8]]) -> Vec<u8> {
    let mut config = b"[safe]\n".to_vec();
    for directory in safe_directories {
        config.extend_from_slice(b"\tdirectory = \"");
        for byte in *directory {
            match byte {
                b'\\' => config.extend_from_slice(b"\\\\"),
                b'"' => config.extend_from_slice(b"\\\""),
                b'\n' => config.extend_from_slice(b"\\n"),
                b'\t' => config.extend_from_slice(b"\\t"),
                other => config.push(*other),
            }
        }
        config.extend_from_slice(b"\"\n");
    }
    config
}

fn stdout_of_success(output: Output, label: &str) -> Result<Vec<u8>, String> {
    if !output.status.success() {
        return Err(format!(
            "`{label}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
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
    stdout_of_success(spawn(command, label)?, label)
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
        let listing = b"system\0/srv/a\0global\0\0local\0/srv/b\0global\0*\0";
        assert_eq!(
            machine_scoped_values(listing),
            Some(vec![&b"/srv/a"[..], &b""[..], &b"*"[..]])
        );
    }

    #[test]
    fn machine_scoped_values_refuse_a_scope_with_no_value() {
        assert_eq!(machine_scoped_values(b"system\0/srv/a\0global\0"), None);
        assert_eq!(machine_scoped_values(b""), Some(Vec::new()));
    }

    fn values_git_reads_back(config: &[u8]) -> Vec<Vec<u8>> {
        let mut file = NamedTempFile::new().expect("temp config");
        file.write_all(config).expect("write config");
        let mut cmd = git_free_of_inherited_state();
        cmd.arg("config").arg("--file").arg(file.path()).args([
            "--null",
            "--get-all",
            "safe.directory",
        ]);
        let listing = checked(cmd, "git config --file").expect("git parses the trust config");
        null_terminated_fields(&listing)
            .map(<[u8]>::to_vec)
            .collect()
    }

    #[test]
    fn trust_config_hands_git_back_every_value_byte_for_byte() {
        let values: [&[u8]; 12] = [
            b"*",
            b"/srv/plain",
            b"/srv/with space/and  two",
            b" leading and trailing ",
            b"/srv/\"quoted\"",
            b"C:\\Users\\operator\\repo",
            b"/srv/ends-in-backslash\\",
            b"",
            b"/srv/#hash;semicolon=equals",
            b"/srv/tab\there/line\nbreak/return\rhere",
            b"/srv/caf\xc3\xa9",
            b"/srv/not-utf8-\xff\xfe",
        ];
        let read_back = values_git_reads_back(&trust_config(&values));
        let expected: Vec<Vec<u8>> = values.iter().map(|value| value.to_vec()).collect();
        assert_eq!(read_back, expected);
    }

    #[test]
    fn trust_config_with_no_values_is_a_config_git_accepts_as_empty() {
        let mut file = NamedTempFile::new().expect("temp config");
        file.write_all(&trust_config(&[])).expect("write config");
        let mut cmd = git_free_of_inherited_state();
        cmd.arg("config").arg("--file").arg(file.path()).args([
            "--null",
            "--get-all",
            "safe.directory",
        ]);
        let output = spawn(cmd, "git config --file").expect("git runs");
        assert_eq!(output.status.code(), Some(KEY_NOT_SET_EXIT));
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
