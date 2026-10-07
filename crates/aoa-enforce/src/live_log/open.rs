//! Containment: acquiring the log's descriptor without letting anything the
//! payload names redirect the write out of `<base>/.aoa/traces/`.
//!
//! Every open in this crate goes through [`open_log`], so the no-follow,
//! nonblocking, and regular-file invariants hold for readers and writers alike.

use std::fs::File;
use std::path::Path;

use super::error::{IoAction, LiveLogError, Result};

/// How the live log is opened. The two modes the hook needs; see [`open_log`]
/// for why the choice is an enum rather than a caller-supplied builder.
pub(super) enum LogAccess {
    Read,
    AppendCreate,
}

/// Open the live log, refusing any path that is not a plain regular file.
///
/// This is not belt-and-braces around the open. A FIFO squatting the path makes
/// `open` *block* until a counterpart appears, so no error is ever produced and
/// the host's fail-closed conversion of an error into a denial never runs — the
/// hook hangs instead, and a hook the host eventually abandons is not a denied
/// write. The bounded lock wait cannot cover this either: it only begins once
/// the open has already returned. A directory or a device node likewise has no
/// business here.
///
/// - A **symlink** at the path is followed by a naive open, so the hook appends
///   its spans into whatever the link names. `O_NOFOLLOW` fails the open instead
///   (`ELOOP`), and does so atomically — an `lstat` check alone would leave a
///   window in which the path is swapped between the check and the open.
/// - A **FIFO** at the path makes a blocking open wait for a peer, hanging the
///   hook (and with it the agent's tool call) before any lock is reached — a DoS
///   that needs no lock contention at all. `O_NONBLOCK` makes the open return
///   rather than wait; on a regular file it has no effect.
///
/// Each Unix directory component below the repository trust root is acquired
/// separately with `openat(O_NOFOLLOW | O_DIRECTORY)`, then the log is opened
/// relative to the acquired traces descriptor. `O_NONBLOCK` still lets a FIFO
/// open succeed when a peer is already attached, so the file type is verified
/// after the fact too, via the descriptor we just opened rather than the path
/// (nothing to race). On a non-Unix host the flags are unavailable and the type
/// check is all there is.
///
/// Callers pick a [`LogAccess`] rather than supplying open flags, so every call
/// goes through the same directory-containment, no-follow, nonblocking, and
/// file-type invariants.
#[cfg(unix)]
pub(super) fn open_log(log: &Path, access: LogAccess) -> Result<File> {
    unix_log::open(log, access)
}

#[cfg(unix)]
mod unix_log {
    use super::super::error::LogPathComponent;
    use super::*;
    use aoa_path_trust::dirfd::{
        map_nofollow_error, open_dir_at, open_or_create_dir_at, open_trust_root,
    };
    use aoa_path_trust::PathTrustError;
    use rustix::fs::{self, Mode, OFlags};
    use std::os::fd::OwnedFd;

    const FILE_MODE: Mode = Mode::RUSR
        .union(Mode::WUSR)
        .union(Mode::RGRP)
        .union(Mode::WGRP)
        .union(Mode::ROTH)
        .union(Mode::WOTH);

    pub(super) fn trust_error(source: PathTrustError) -> LiveLogError {
        match source {
            PathTrustError::Io { path, source } => LiveLogError::Io {
                action: IoAction::Open,
                path,
                source,
            },
            PathTrustError::UnsafePath { path } => LiveLogError::SymlinkRefused { path },
            refusal => LiveLogError::PathRefused(refusal),
        }
    }

    fn log_parts(log: &Path) -> Result<(&Path, &Path, &Path, &std::ffi::OsStr)> {
        let traces_dir = log.parent().ok_or_else(|| LiveLogError::MalformedLogPath {
            component: LogPathComponent::TracesDir,
            path: log.to_path_buf(),
        })?;
        let aoa_dir = traces_dir
            .parent()
            .ok_or_else(|| LiveLogError::MalformedLogPath {
                component: LogPathComponent::AoaDir,
                path: log.to_path_buf(),
            })?;
        let repo = aoa_dir
            .parent()
            .ok_or_else(|| LiveLogError::MalformedLogPath {
                component: LogPathComponent::RepoRoot,
                path: log.to_path_buf(),
            })?;
        let name = log
            .file_name()
            .ok_or_else(|| LiveLogError::MalformedLogPath {
                component: LogPathComponent::FileName,
                path: log.to_path_buf(),
            })?;
        Ok((repo, aoa_dir, traces_dir, name))
    }

    fn open_traces_dir(
        repo: &Path,
        aoa_dir: &Path,
        traces_dir: &Path,
        access: LogAccess,
    ) -> Result<OwnedFd> {
        let acquire = |parent: &OwnedFd, name: &str, path: &Path| {
            match access {
                LogAccess::Read => open_dir_at(parent, name, path),
                LogAccess::AppendCreate => open_or_create_dir_at(parent, name, path),
            }
            .map_err(trust_error)
        };
        let repo_fd = open_trust_root(repo).map_err(trust_error)?;
        let aoa_fd = acquire(&repo_fd, ".aoa", aoa_dir)?;
        acquire(&aoa_fd, "traces", traces_dir)
    }

    pub(super) fn open(log: &Path, access: LogAccess) -> Result<File> {
        let flags = match access {
            LogAccess::Read => OFlags::RDONLY,
            LogAccess::AppendCreate => OFlags::RDWR | OFlags::CREATE | OFlags::APPEND,
        } | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC;
        let (repo, aoa_dir, traces_dir, name) = log_parts(log)?;
        let traces_fd = open_traces_dir(repo, aoa_dir, traces_dir, access)?;
        let fd = fs::openat(&traces_fd, name, flags, FILE_MODE).map_err(|source| {
            trust_error(map_nofollow_error(&traces_fd, name, log, source.into()))
        })?;
        let file = File::from(fd);
        let file_type = file
            .metadata()
            .map_err(|err| LiveLogError::io(log, IoAction::Stat, err))?
            .file_type();
        if !file_type.is_file() {
            return Err(LiveLogError::NotRegularFile {
                path: log.to_path_buf(),
                file_type,
            });
        }
        Ok(file)
    }
}

#[cfg(not(unix))]
pub(super) fn open_log(log: &Path, access: LogAccess) -> Result<File> {
    let mut options = File::options();
    match access {
        LogAccess::Read => options.read(true),
        LogAccess::AppendCreate => options.read(true).create(true).append(true),
    };
    let file = options
        .open(log)
        .map_err(|err| LiveLogError::io(log, IoAction::Open, err))?;
    let file_type = file
        .metadata()
        .map_err(|err| LiveLogError::io(log, IoAction::Stat, err))?
        .file_type();
    if !file_type.is_file() {
        return Err(LiveLogError::NotRegularFile {
            path: log.to_path_buf(),
            file_type,
        });
    }
    Ok(file)
}

#[cfg(not(unix))]
pub(super) fn create_traces_dir(path: &Path) -> Result<()> {
    // Unix mode bits have no portable Windows equivalent. Keep the platform's
    // inherited ACL here; the final log target is still opened atomically and
    // must be a regular file.
    std::fs::create_dir_all(path).map_err(|err| LiveLogError::io(path, IoAction::Create, err))
}

// Every test here plants something only a Unix host can plant — a symlink, a
// FIFO, a mode bit — so the whole group is gated rather than each test. Gating
// them individually would leave the imports behind on a non-unix host, where
// they are then unused and fail the crate's `-D warnings` build.
#[cfg(all(test, unix))]
mod tests {
    use super::super::error::IoAction;
    use super::super::{append_span, read_spans, LiveLogError};
    use super::{open_log, LogAccess};
    use aoa_trace::SpanType;
    use serde_json::Map;
    use std::time::Duration;

    #[test]
    fn a_trust_refusal_keeps_the_reason_it_was_refused_for() {
        use super::unix_log::trust_error;
        use aoa_path_trust::PathTrustError;
        use std::path::PathBuf;

        let path = PathBuf::from("/repo/.aoa/traces");

        let err = trust_error(PathTrustError::TooManyLinks {
            path: path.clone(),
            limit: 40,
        });
        assert!(
            matches!(&err, LiveLogError::PathRefused(PathTrustError::TooManyLinks { path: p, limit: 40 }) if *p == path),
            "{err:?}"
        );
        assert!(err.to_string().contains("more than 40 links"), "{err}");

        let err = trust_error(PathTrustError::NotRegularFile { path: path.clone() });
        assert!(
            matches!(&err, LiveLogError::PathRefused(PathTrustError::NotRegularFile { path: p }) if *p == path),
            "{err:?}"
        );

        let err = trust_error(PathTrustError::UnsafePath { path: path.clone() });
        assert!(
            matches!(&err, LiveLogError::SymlinkRefused { path: p } if *p == path),
            "{err:?}"
        );

        let err = trust_error(PathTrustError::Io {
            path: path.clone(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        });
        assert!(
            matches!(&err, LiveLogError::Io { action: IoAction::Open, path: p, source } if *p == path && source.kind() == std::io::ErrorKind::PermissionDenied),
            "{err:?}"
        );
    }

    #[test]
    fn an_opened_log_is_closed_on_exec() {
        use rustix::io::{fcntl_getfd, FdFlags};
        use std::os::fd::AsFd;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-cloexec.jsonl");

        for access in [LogAccess::AppendCreate, LogAccess::Read] {
            let file = open_log(&log, access).unwrap();
            let flags = fcntl_getfd(file.as_fd()).unwrap();
            assert!(
                flags.contains(FdFlags::CLOEXEC),
                "the log descriptor would leak into a child process"
            );
        }
    }

    #[test]
    fn append_creates_private_trace_directories() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-private.jsonl");

        append_span(&log, SpanType::TestRun, Map::new()).unwrap();

        for path in [dir.path().join(".aoa"), dir.path().join(".aoa/traces")] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o077,
                0,
                "{} must not grant group or other access",
                path.display()
            );
        }
    }

    /// A symlink already sitting at the log path must not be followed.
    /// Reproduced before the fix: the appended span landed in the victim file.
    #[test]
    fn a_planted_symlink_is_refused_and_the_victim_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        // Keep the victim valid as an empty log so a naive open reaches the
        // append; malformed content would make the read path fail first.
        std::fs::write(&victim, "").unwrap();

        let log = dir.path().join(".aoa/traces/live-unknown.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&victim, &log).unwrap();

        let err = append_span(&log, SpanType::TestRun, Map::new()).unwrap_err();
        assert!(
            matches!(err, LiveLogError::SymlinkRefused { ref path } if *path == log),
            "the refusal must name the plant it refused, got: {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "",
            "the span must not have been appended through the symlink"
        );
        read_spans(&log).unwrap_err();
    }

    #[test]
    fn a_planted_fifo_is_refused_instead_of_hanging() {
        use std::ffi::CString;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-unknown.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let raw = CString::new(log.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0, "mkfifo");

        let (tx, rx) = std::sync::mpsc::channel();
        let target = log.clone();
        std::thread::spawn(move || {
            let _ = tx.send(append_span(&target, SpanType::TestRun, Map::new()));
        });
        let outcome = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the open must return rather than block on the FIFO");
        let err = outcome.expect_err("a FIFO at the log path must be an error");
        assert!(
            matches!(err, LiveLogError::NotRegularFile { ref path, .. } if *path == log),
            "must refuse the FIFO as a non-regular file, got: {err:?}"
        );
    }

    #[test]
    fn a_trace_directory_that_cannot_be_created_reports_a_failed_open() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::set_permissions(&repo, std::fs::Permissions::from_mode(0o555)).unwrap();
        let sealed = std::fs::create_dir(repo.join("probe")).is_err();
        let log = repo.join(".aoa/traces/live-sealed.jsonl");

        let outcome = append_span(&log, SpanType::TestRun, Map::new());

        std::fs::set_permissions(&repo, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !sealed {
            eprintln!(
                "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process creates \
                 an entry in a mode-555 directory, so it cannot be denied the mkdir"
            );
            return;
        }
        let err = outcome.expect_err("a read-only repository cannot hold a new .aoa");
        assert!(
            matches!(
                err,
                LiveLogError::Io { action: IoAction::Open, ref path, ref source }
                    if *path == repo.join(".aoa")
                        && source.kind() == std::io::ErrorKind::PermissionDenied
            ),
            "the mkdir refusal must surface as a failed open of .aoa, got: {err:?}"
        );
        assert_eq!(
            std::fs::read_dir(&repo).unwrap().count(),
            0,
            "nothing may be created beneath the sealed repository"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_planted_symlink_with_a_non_utf8_name_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "").unwrap();
        let traces = dir.path().join(".aoa/traces");
        std::fs::create_dir_all(&traces).unwrap();
        let log = traces.join(OsStr::from_bytes(b"live-\xff.jsonl"));
        std::os::unix::fs::symlink(&victim, &log).unwrap();

        let err = append_span(&log, SpanType::TestRun, Map::new()).unwrap_err();
        assert!(
            matches!(err, LiveLogError::SymlinkRefused { ref path } if *path == log),
            "the refusal must name the plant it refused, got: {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "",
            "the span must not have been appended through the symlink"
        );
    }
}
