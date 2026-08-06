//! Containment: acquiring the log's descriptor without letting anything the
//! payload names redirect the write out of `<base>/.aoa/traces/`.
//!
//! Every open in this crate goes through [`open_log`], so the no-follow,
//! nonblocking, and regular-file invariants hold for readers and writers alike.

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

/// Whether an [`open_log`] failure was "the log does not exist yet", which is the
/// ordinary state before the first span is written. Checked through the error
/// chain because `open_log` attaches context to the underlying [`std::io::Error`].
/// Every other failure — a symlink, a FIFO, a permission problem — stays an error.
pub(super) fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

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
    use super::*;
    use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
    use rustix::io::Errno;
    use std::ffi::OsStr;
    use std::os::fd::{AsFd, OwnedFd};

    const DIR_MODE: Mode = Mode::RWXU;
    const FILE_MODE: Mode = Mode::RUSR
        .union(Mode::WUSR)
        .union(Mode::RGRP)
        .union(Mode::WGRP)
        .union(Mode::ROTH)
        .union(Mode::WOTH);

    fn io_error(path: &Path, action: &str, source: Errno) -> anyhow::Error {
        let source: std::io::Error = source.into();
        anyhow!(source).context(format!("failed to {action} {}", path.display()))
    }

    fn is_symlink_at(parent: impl AsFd, name: &OsStr) -> bool {
        fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
            .unwrap_or(false)
    }

    fn map_nofollow_error(
        parent: impl AsFd,
        name: &OsStr,
        path: &Path,
        source: Errno,
    ) -> anyhow::Error {
        if source == Errno::LOOP || is_symlink_at(parent, name) {
            anyhow!("refusing to follow symlink at {}", path.display())
        } else {
            io_error(path, "open", source)
        }
    }

    fn open_dir_at(parent: impl AsFd, name: &OsStr, path: &Path) -> Result<OwnedFd> {
        fs::openat(
            parent.as_fd(),
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|source| map_nofollow_error(parent, name, path, source))
    }

    fn open_or_create_dir_at(parent: impl AsFd, name: &OsStr, path: &Path) -> Result<OwnedFd> {
        match open_dir_at(&parent, name, path) {
            Ok(fd) => Ok(fd),
            Err(err) if is_not_found(&err) => {
                match fs::mkdirat(parent.as_fd(), name, DIR_MODE) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(source) => return Err(io_error(path, "create", source)),
                }
                open_dir_at(parent, name, path)
            }
            Err(err) => Err(err),
        }
    }

    fn log_parts(log: &Path) -> Result<(&Path, &Path, &Path, &OsStr)> {
        let traces_dir = log
            .parent()
            .ok_or_else(|| anyhow!("span log has no traces directory: {}", log.display()))?;
        let aoa_dir = traces_dir
            .parent()
            .ok_or_else(|| anyhow!("span log has no .aoa directory: {}", log.display()))?;
        let repo = aoa_dir
            .parent()
            .ok_or_else(|| anyhow!("span log has no repository root: {}", log.display()))?;
        let name = log
            .file_name()
            .ok_or_else(|| anyhow!("span log has no file name: {}", log.display()))?;
        Ok((repo, aoa_dir, traces_dir, name))
    }

    fn open_traces_dir(
        repo: &Path,
        aoa_dir: &Path,
        traces_dir: &Path,
        access: LogAccess,
    ) -> Result<OwnedFd> {
        // The caller-selected repository is the trust root. Every component
        // below it is acquired relative to a stable descriptor.
        let repo_fd = fs::open(repo, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())
            .map_err(|source| io_error(repo, "open", source))?;
        let aoa_fd = match access {
            LogAccess::Read => open_dir_at(&repo_fd, OsStr::new(".aoa"), aoa_dir)?,
            LogAccess::AppendCreate => {
                open_or_create_dir_at(&repo_fd, OsStr::new(".aoa"), aoa_dir)?
            }
        };
        match access {
            LogAccess::Read => open_dir_at(&aoa_fd, OsStr::new("traces"), traces_dir),
            LogAccess::AppendCreate => {
                open_or_create_dir_at(&aoa_fd, OsStr::new("traces"), traces_dir)
            }
        }
    }

    pub(super) fn open(log: &Path, access: LogAccess) -> Result<File> {
        let flags = match access {
            LogAccess::Read => OFlags::RDONLY,
            LogAccess::AppendCreate => OFlags::RDWR | OFlags::CREATE | OFlags::APPEND,
        } | OFlags::NOFOLLOW
            | OFlags::NONBLOCK;
        let (repo, aoa_dir, traces_dir, name) = log_parts(log)?;
        let traces_fd = open_traces_dir(repo, aoa_dir, traces_dir, access)?;
        let fd = fs::openat(&traces_fd, name, flags, FILE_MODE)
            .map_err(|source| map_nofollow_error(&traces_fd, name, log, source))?;
        let file = File::from(fd);
        let file_type = file
            .metadata()
            .with_context(|| format!("failed to stat {}", log.display()))?
            .file_type();
        if !file_type.is_file() {
            return Err(anyhow!(
                "refusing to use {}: the span log must be a regular file, found {file_type:?}",
                log.display()
            ));
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
        .with_context(|| format!("failed to open {}", log.display()))?;
    let file_type = file
        .metadata()
        .with_context(|| format!("failed to stat {}", log.display()))?
        .file_type();
    if !file_type.is_file() {
        return Err(anyhow!(
            "refusing to use {}: the span log must be a regular file, found {file_type:?}",
            log.display()
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
pub(super) fn create_traces_dir(path: &Path) -> Result<()> {
    // Unix mode bits have no portable Windows equivalent. Keep the platform's
    // inherited ACL here; the final log target is still opened atomically and
    // must be a regular file.
    std::fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
}

// Every test here plants something only a Unix host can plant — a symlink, a
// FIFO, a mode bit — so the whole group is gated rather than each test. Gating
// them individually would leave the imports behind on a non-unix host, where
// they are then unused and fail the crate's `-D warnings` build.
#[cfg(all(test, unix))]
mod tests {
    use super::super::{append_span, read_spans};
    use aoa_trace::SpanType;
    use serde_json::Map;
    use std::time::Duration;

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
            !format!("{err:#}").is_empty(),
            "the refusal must carry a diagnosable message"
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
            let _ = tx.send(append_span(&target, SpanType::TestRun, Map::new()).is_err());
        });
        let refused = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the open must return rather than block on the FIFO");
        assert!(refused, "a FIFO at the log path must be an error");
    }
}
