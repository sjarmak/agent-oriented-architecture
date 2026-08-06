//! The durable live-span log: the append-only, crash-consistent store the
//! enforcement gates read their input from and write their outcomes to.
//!
//! This module owns the log that [`reproduction_gate`](crate::reproduction_gate)
//! decides over, so the invariants that make that decision trustworthy are
//! stated here rather than in whichever host happens to run the hook:
//!
//! - **Monotonic `seq`.** Every append derives its sequence number inside an
//!   exclusive advisory lock held across the read-modify-write, so concurrent
//!   writers cannot duplicate or decrease it. A decreasing `seq` fails
//!   `validate_trace` for the whole repository against a file with no repair
//!   path, so this is a correctness property, not tidiness.
//! - **Bounded waiting.** Both lock modes give up after a deadline rather than
//!   inheriting another process's stall. Hooks run synchronously in an agent's
//!   tool path; an unbounded wait freezes the session.
//! - **Containment.** The log is opened relative to descriptors acquired one
//!   component at a time below the caller-supplied trust root, refusing
//!   symlinks and non-regular files, so nothing the payload names can redirect
//!   the write out of `<base>/.aoa/traces/`.
//! - **All-or-nothing appends.** A partial write is rolled back under the lock,
//!   and a torn tail left by an earlier crash is repaired before the next
//!   append rather than spliced onto.
//!
//! ## Trust root
//!
//! [`LiveLog::for_session`] takes the repository root as `base` and treats it as
//! already-resolved trust: it is opened directly, and only the `.aoa` and
//! `traces` components below it are acquired with `O_NOFOLLOW`. Resolving an
//! untrusted path *into* a repository root is the caller's job, deliberately not
//! this module's. The session id needs no such care — it is coerced to a single
//! safe filename component here.

use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};

use aoa_trace::{validate_single_component, Span, SpanSource, SpanType};

/// How long a hook waits for the span log's lock before failing. Generous
/// against real contention (the lock spans one read and one append) and short
/// enough that a wedged holder cannot stall the session.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll interval while waiting. Short enough to be invisible under normal
/// contention, long enough not to spin a core against a wedged holder.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(5);

/// Bytes of the log's tail [`next_seq`] reads per attempt. Comfortably covers a
/// *typical* span line (60–300 bytes) so the common append needs exactly one
/// read; a longer final line widens the window rather than being truncated into
/// a parse error. Undersizing costs an extra read round-trip, oversizing costs
/// a bigger copy on every append, and 8 KiB is the slack side of that trade.
const SEQ_TAIL_WINDOW: u64 = 8 * 1024;

/// Ceiling on that widening — the same order of magnitude the sibling readers
/// use (`aoa_trace`'s `MAX_TRACE_BYTES`, `aoa-observe-shim`'s
/// `MAX_CORPUS_FILE_BYTES`), though nothing links the three; growing the read
/// past it would trade away the bound `next_seq` exists to establish.
///
/// Deliberately far above a typical span line, which runs 60–300 bytes, because
/// a line is *not* bounded by anything this layer controls: a host takes the
/// write target straight from its hook payload, so a 20 KiB path yields a 20 KiB
/// line. Widening therefore has to reach well past [`SEQ_TAIL_WINDOW`] — a
/// single unwidened read would refuse every append after one such line, wedging
/// recording for the rest of the session.
const MAX_SEQ_TAIL_BYTES: u64 = 16 * 1024 * 1024;

/// A torn final line that an append discarded before writing, with the number of
/// bytes dropped.
///
/// Returned rather than printed. A repair is a fact about the log, so this
/// module states it; how an operator hears about it — stderr, a structured
/// event, nothing at all — belongs to whichever host is driving, and a library
/// that writes to a terminal takes that choice away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TornTailRepair {
    /// Bytes discarded from the unterminated tail.
    pub discarded_bytes: u64,
}

/// The append-only span log for one session, under a repository's ignored
/// `.aoa/traces/` tree.
///
/// Holds the resolved path, not an open descriptor: each operation opens its own
/// handle because the advisory lock is per open-file-description and must be
/// held across exactly one read-modify-write. A long-lived handle would either
/// hold the lock across unrelated work or need re-locking anyway.
#[derive(Debug)]
pub struct LiveLog {
    path: PathBuf,
}

impl LiveLog {
    /// The log for `session_id` under the already-resolved repository root
    /// `base`. Performs no IO; the session id is coerced to a single safe
    /// filename component so a hostile value cannot traverse out of the traces
    /// directory.
    #[must_use]
    pub fn for_session(base: &Path, session_id: &str) -> Self {
        LiveLog {
            path: live_log_path(base, session_id),
        }
    }

    /// The resolved log path. Exposed so a host can name the file in a
    /// diagnostic without re-deriving it.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every span recorded so far, empty when nothing has been written yet.
    /// Fails loud on a corrupt line.
    pub fn read_spans(&self) -> Result<Vec<Span>> {
        read_spans(&self.path)
    }

    /// Append a native span of `span_type` carrying `attributes`, numbered one
    /// past the log's last. Returns any torn tail repaired on the way in.
    pub fn append(
        &self,
        span_type: SpanType,
        attributes: Map<String, Value>,
    ) -> Result<Option<TornTailRepair>> {
        append_span(&self.path, span_type, attributes)
    }

    /// [`LiveLog::append`] for a caller that builds the whole span, receiving
    /// the assigned sequence number.
    ///
    /// `build` runs inside the locked region: deriving the number means reading
    /// the log's tail, and a caller that built its span beforehand would
    /// reintroduce the race this lock exists to prevent.
    pub fn append_with(&self, build: impl FnOnce(u64) -> Span) -> Result<Option<TornTailRepair>> {
        append_span_with(&self.path, build)
    }
}

/// Resolve the append-only live-log path for this session, under the ignored
/// `.aoa/traces/` tree. The session id is sanitized to a bare filename token so
/// a hostile payload cannot traverse out of the traces directory. The final
/// component is opened safely by [`open_log`].
fn live_log_path(base: &Path, session_id: &str) -> PathBuf {
    let session = sanitize_session(session_id);
    base.join(".aoa")
        .join("traces")
        .join(format!("live-{session}.jsonl"))
}

/// Reduce a session id to `[A-Za-z0-9_-]`, collapsing everything else. Guarantees
/// the value is a single safe path component (no separators, no `..`). Empty or
/// fully-stripped ids become `unknown` so a log still has a stable home.
fn sanitize_session(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    let candidate = if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    };
    if validate_single_component(&candidate).is_ok() {
        candidate
    } else {
        // Defensive fallback: the coercion above currently emits only the safe
        // ASCII alphabet, but the shared validator remains the postcondition if
        // that alphabet changes later.
        "unknown".to_string()
    }
}

/// Whether an [`open_log`] failure was "the log does not exist yet", which is the
/// ordinary state before the first span is written. Checked through the error
/// chain because `open_log` attaches context to the underlying [`std::io::Error`].
/// Every other failure — a symlink, a FIFO, a permission problem — stays an error.
fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// How the live log is opened. The two modes the hook needs; see [`open_log`]
/// for why the choice is an enum rather than a caller-supplied builder.
enum LogAccess {
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
fn open_log(log: &Path, access: LogAccess) -> Result<File> {
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
fn open_log(log: &Path, access: LogAccess) -> Result<File> {
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

/// Read the live log into spans, tolerating a missing file (no reproduction yet)
/// but failing loud on a corrupt line — a malformed log is a real defect, not
/// something to silently skip.
///
/// Taken under a bounded **shared** lock, so the read never overlaps an in-flight
/// [`append_span_with`]. Without it a reader could observe a half-written final
/// line, and both ways of handling that are wrong: parsing it fails the hook on
/// input that is merely in flight, while skipping it feeds [`reproduction_gate`]
/// a log missing the very `test.run` being recorded, which blocks the agent's
/// write for a reason that is not true (aoa-wew0).
fn read_spans(log: &Path) -> Result<Vec<Span>> {
    read_spans_within(log, LOCK_TIMEOUT)
}

/// [`read_spans`], with the lock timeout supplied. Exists for the same reason
/// [`append_span_within`] does: a test must exercise the real read path,
/// including that it acquires the lock boundedly, without waiting a full
/// [`LOCK_TIMEOUT`].
fn read_spans_within(log: &Path, lock_timeout: Duration) -> Result<Vec<Span>> {
    let mut file = match open_log(log, LogAccess::Read) {
        Ok(file) => file,
        Err(err) if is_not_found(&err) => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    lock_shared_bounded(&file, log, lock_timeout)?;
    read_spans_from(&mut file, log)
}

/// Read an already-open, already-locked log handle into spans, failing loud on
/// any corrupt line.
///
/// Takes the handle rather than the path so the append path can reuse it against
/// the descriptor it already holds the exclusive lock on, rather than opening a
/// second descriptor to re-read the same bytes.
fn read_spans_from(file: &mut File, log: &Path) -> Result<Vec<Span>> {
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .with_context(|| format!("failed to read {}", log.display()))?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_span_line(line.as_bytes(), log))
        .collect()
}

/// Append a fresh span of `span_type` with `attributes`, numbered one past the
/// log's last `seq` so ordering stays monotonic.
fn append_span(
    log: &Path,
    span_type: SpanType,
    attributes: Map<String, Value>,
) -> Result<Option<TornTailRepair>> {
    append_span_with(log, |seq| Span {
        span_type,
        source: SpanSource::Native,
        seq,
        attributes,
    })
}

/// Append one span, assigning its `seq` and writing it under an exclusive lock.
///
/// `build` receives the next sequence number, so the number is derived *inside*
/// the locked region. That is the whole point of the closure: deriving `seq`
/// means reading the log's tail, which together with the append is a
/// read-modify-write. Every mutation now drives two of these (intent at
/// `PreToolUse`, then an outcome), so unsynchronized racers were routine, and
/// they produced duplicate and even *decreasing* `seq` values. A decreasing
/// `seq` is not a cosmetic defect: `validate_trace` rejects the trace,
/// `load_corpus` propagates the error, and `aoa audit` then fails for the whole
/// repo — against an append-only log with no repair path. A caller that built
/// its span before calling would reintroduce exactly that race.
///
/// The critical section opens no second descriptor: [`next_seq`] reads through
/// the very handle that holds the lock. That was not always so — deriving `seq`
/// used to mean `read_to_string` on the same path, a second open inside the
/// locked region, which constrained the lock to one held per *open file
/// description* ([`File::try_lock`]) because a classic POSIX record lock
/// (`fcntl(F_SETLK)`) is per-process-per-inode and closing that second
/// descriptor would have dropped the lock mid-update. The constraint is gone
/// with the second open; `try_lock` stays because per-OFD is the right
/// semantics for a handle this function owns.
///
/// That per-description property is also why `seq` is derived by reading
/// **this** handle rather than reopening the path: a second descriptor would be
/// a separate lock holder, and [`read_spans`] takes a shared lock, so reading
/// through it would contend with the exclusive lock this function already holds
/// and die at [`LOCK_TIMEOUT`].
///
/// Readers take a shared lock, so writer-vs-writer and reader-vs-writer are both
/// serialized here; see [`read_spans`] for why an unlocked read was not merely
/// noisy but could produce a wrong gate decision.
fn append_span_with(log: &Path, build: impl FnOnce(u64) -> Span) -> Result<Option<TornTailRepair>> {
    append_span_within(log, LOCK_TIMEOUT, build)
}

/// [`append_span_with`], with the lock timeout supplied.
///
/// Exists so a test can exercise the real append path — including that it
/// acquires the lock *boundedly* — without waiting a full [`LOCK_TIMEOUT`].
/// Testing [`lock_exclusive_bounded`] alone would leave the wiring uncovered:
/// swapping this call for a blocking acquisition reintroduces the session-freeze
/// hazard while every test still passes.
///
/// A repair is reported only on the success path, because `Result` carries one
/// or the other. Returning the fact instead of printing it from inside the lock
/// is what buys this module its freedom from an output channel, and the trade is
/// narrow: the append fails only on an oversized line or a write error, and both
/// return an error naming this same log, so the operator is not left thinking it
/// is healthy. What is lost in that window is the byte count of a truncation
/// that did happen.
fn append_span_within(
    log: &Path,
    lock_timeout: Duration,
    build: impl FnOnce(u64) -> Span,
) -> Result<Option<TornTailRepair>> {
    #[cfg(not(unix))]
    if let Some(parent) = log.parent() {
        create_traces_dir(parent)?;
    }
    // Bound to a named local, never a temporary: dropping the `File` closes the
    // descriptor and releases the lock, so a temporary would unlock immediately
    // and leave the read-modify-write below unguarded.
    //
    // `read` is requested alongside `append` so `next_seq` can derive the
    // sequence number through this same descriptor. `O_APPEND` repositions every
    // write to end-of-file atomically, so the seeking that read does cannot
    // misplace the append below.
    let mut file = open_log(log, LogAccess::AppendCreate)?;
    lock_exclusive_bounded(&file, log, lock_timeout)?;
    let repair =
        repair_torn_tail(&mut file, log)?.map(|discarded_bytes| TornTailRepair { discarded_bytes });

    let end = file
        .metadata()
        .with_context(|| format!("failed to stat {}", log.display()))?
        .len();
    let span = build(next_seq(&mut file, log, MAX_SEQ_TAIL_BYTES)?);
    let mut line = serde_json::to_string(&span).context("failed to serialize span")?;
    line.push('\n');

    // Never write a line this log cannot read back. `next_seq` resolves the last
    // line by widening its tail read to at most `MAX_SEQ_TAIL_BYTES`, so a
    // longer line would append once and then refuse every later append for the
    // rest of the session — the log poisoning itself. Reachable because a host
    // takes the write target straight from its hook payload, which nothing
    // bounds.
    if line.len() as u64 > MAX_SEQ_TAIL_BYTES {
        return Err(anyhow!(
            "refusing to append a {}-byte span line to {}: it exceeds the \
             {MAX_SEQ_TAIL_BYTES}-byte tail read, so no later append could \
             derive its sequence number",
            line.len(),
            log.display()
        ));
    }

    // A failed `write_all` may still have written a prefix — `ENOSPC` mid-line
    // is the ordinary case. That prefix is a torn tail, which `next_seq` then
    // refuses forever, so a transient disk-full would permanently stop
    // recording. Roll back to the pre-append length while the lock is still
    // held; the append is all-or-nothing from any other writer's view.
    if let Err(err) = file.write_all(line.as_bytes()) {
        let _ = file.set_len(end);
        return Err(anyhow!(err)).with_context(|| format!("failed to append to {}", log.display()));
    }
    Ok(repair)
}

#[cfg(not(unix))]
fn create_traces_dir(path: &Path) -> Result<()> {
    // Unix mode bits have no portable Windows equivalent. Keep the platform's
    // inherited ACL here; the final log target is still opened atomically and
    // must be a regular file.
    std::fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
}

/// Remove an unterminated final fragment while holding the writer's exclusive
/// lock. A complete log is untouched; a file containing only a fragment is
/// reset to empty. The backward scan has no corpus-sized allocation and runs
/// only on the exceptional recovery path.
fn repair_torn_tail(file: &mut File, log: &Path) -> Result<Option<u64>> {
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat {}", log.display()))?
        .len();
    if len == 0 || read_at(file, log, len - 1, 1)? == b"\n" {
        return Ok(None);
    }

    let mut end = len;
    loop {
        let start = end.saturating_sub(SEQ_TAIL_WINDOW);
        let buf = read_at(file, log, start, end - start)?;
        if let Some(index) = buf.iter().rposition(|byte| *byte == b'\n') {
            let retained = start + index as u64 + 1;
            file.set_len(retained)
                .with_context(|| format!("failed to repair torn tail in {}", log.display()))?;
            return Ok(Some(len - retained));
        }
        if start == 0 {
            file.set_len(0)
                .with_context(|| format!("failed to repair torn tail in {}", log.display()))?;
            return Ok(Some(len));
        }
        end = start;
    }
}

/// The sequence number the next span should carry: one past the log's last.
///
/// Reads only the log's tail. Deriving `seq` by counting the whole log — which
/// is what this replaced — made every hook invocation O(n) in the log's length,
/// and since that read happens inside [`append_span_within`]'s exclusive lock,
/// a long session's writes were both quadratic and serialized. Hook latency
/// lands directly in the agent's tool path, so that cost is agent-visible.
///
/// *Last + 1*, not *count*, and the difference is correctness rather than
/// convenience. `validate_trace` compares **adjacent** spans, rejecting only a
/// `seq` lower than its predecessor's. On a log carrying a gap — which the
/// pre-lock races described on [`append_span_with`] really did produce — the
/// count is smaller than the last `seq`, so a count-derived append writes a
/// *decrease*, and that wedges `aoa audit` for the whole repo against an
/// append-only file with no repair path. Last + 1 recovers from any tail.
///
/// Only the final line is parsed, so corruption earlier in the log no longer
/// fails this path — nor does invalid UTF-8 outside the window, which the
/// whole-file `read_to_string` used to reject up front. That narrowing is
/// deliberate: the corpus re-validates at ingest, and a host running the
/// reproduction gate still reads and parses the log in full on every mutation
/// *when that gate is on* — a policy that sets `reproduction_required: false`
/// skips the read, leaving ingest as the only check. Failing here instead
/// would freeze all further recording on a log with any historical bad line.
fn next_seq(file: &mut File, log: &Path, max_tail: u64) -> Result<u64> {
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat {}", log.display()))?
        .len();
    if len == 0 {
        return Ok(0);
    }

    // One bound for every read, the first included. Deriving it once is what
    // makes the cap real: clamping only at the widen step left the *initial*
    // read bounded by `len` alone, so a `max_tail` below one window was
    // exceeded before any widening was even considered.
    let max_window = len.min(max_tail);
    let mut window = SEQ_TAIL_WINDOW.min(max_window);
    loop {
        let start = len - window;
        let buf = read_at(file, log, start, window)?;
        // Every window ends at EOF (`start + window == len`), so the buffer's
        // last byte is the file's. An append-only log always ends with the
        // newline `append_span_within` wrote; a missing one means the previous
        // append was torn, and `O_APPEND` would splice this span onto that
        // partial line. Checked before trimming, so a trailing space still
        // reads as torn.
        if buf.last() != Some(&b'\n') {
            return Err(anyhow!(
                "{} has no trailing newline, so its last line is a torn write; \
                 appending would splice this span onto it",
                log.display()
            ));
        }
        // Trimming the end collapses the final newline and any blank lines
        // after it, so whatever follows the last remaining newline is a
        // non-empty line — the same blank tolerance `read_spans` has.
        let trimmed = buf.trim_ascii_end();
        match trimmed.iter().rposition(|byte| *byte == b'\n') {
            Some(cut) => return succ(parse_seq(&trimmed[cut + 1..], log)?, log),
            // Nothing before the window: the log is nothing but blanks, or
            // else it is a single line.
            None if start == 0 => {
                return match trimmed.is_empty() {
                    true => Ok(0),
                    false => succ(parse_seq(trimmed, log)?, log),
                }
            }
            // The last line straddles the window's start; widen and retry
            // until `max_window` leaves no room to grow.
            None => {
                let grown = (window * 2).min(max_window);
                if grown <= window {
                    return Err(anyhow!(
                        "the last line of {} exceeds the {max_tail}-byte tail read",
                        log.display()
                    ));
                }
                window = grown;
            }
        }
    }
}

/// Read exactly `len` bytes at `offset`. Seeks from the start rather than the
/// end: `SeekFrom::End` with a negative offset larger than the file is an error,
/// not a clamp, so a log shorter than one window would fail there.
fn read_at(file: &mut File, log: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("failed to seek {}", log.display()))?;
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)
        .with_context(|| format!("failed to read {}", log.display()))?;
    Ok(buf)
}

/// The `seq` of one serialized span line.
fn parse_seq(line: &[u8], log: &Path) -> Result<u64> {
    Ok(parse_span_line(line, log)?.seq)
}

/// One past `seq`, refusing to wrap.
///
/// A plain `+ 1` panics on overflow only in debug builds; release builds wrap
/// `u64::MAX` to `0`, which appends a *decreasing* `seq` — the one outcome this
/// whole locked read-modify-write exists to prevent, and unrepairable on an
/// append-only log. Exhaustion is not reachable by counting (it would take more
/// appends than the disk could hold) but a single corrupt or hand-written line
/// carrying `u64::MAX` reaches it immediately.
fn succ(seq: u64, log: &Path) -> Result<u64> {
    seq.checked_add(1).ok_or_else(|| {
        anyhow!(
            "the last span in {} carries seq {seq}, the maximum; the next \
             sequence number would wrap to zero and make the log decreasing",
            log.display()
        )
    })
}

/// Deserialize one span line, failing loud on a corrupt one.
///
/// Shared by [`read_spans`] and [`parse_seq`] so the two paths that read this
/// file report the same corruption identically. They used to spell the context
/// separately, which made the wording a convention rather than a fact: adding a
/// byte offset or a repair hint to one would silently leave the other behind.
fn parse_span_line(line: &[u8], log: &Path) -> Result<Span> {
    serde_json::from_slice(line).with_context(|| format!("corrupt span line in {}", log.display()))
}

/// Take the log's exclusive lock, giving up after `timeout` rather than waiting
/// forever.
///
/// Hooks run synchronously in the agent's tool path, so a blocking acquisition
/// hands any process that stalls while holding this lock — stopped, or on a
/// wedged network mount — the ability to freeze every later hook in the session
/// with no recovery but killing it by hand. The lock is only ever held across
/// one read and one append, so contention resolves in microseconds; waiting
/// seconds means something is already wrong, and reporting that is strictly
/// better than inheriting the stall.
///
/// Giving up is a real error, never a silent fallback to an unlocked append:
/// that would trade a visible failure for the corrupt `seq` this lock exists to
/// prevent.
fn lock_exclusive_bounded(file: &File, log: &Path, timeout: Duration) -> Result<()> {
    lock_bounded(file, log, timeout, File::try_lock)
}

/// [`lock_exclusive_bounded`] for a reader: takes the log's shared lock, so
/// concurrent readers do not exclude each other but none of them overlaps an
/// in-flight append. Bounded for the same reason the exclusive acquisition is —
/// a reader that inherits a wedged writer's stall freezes the session just as
/// thoroughly as a writer that does.
fn lock_shared_bounded(file: &File, log: &Path, timeout: Duration) -> Result<()> {
    lock_bounded(file, log, timeout, File::try_lock_shared)
}

/// The bounded-acquisition loop both lock modes share; `try_acquire` is the only
/// difference between them.
fn lock_bounded(
    file: &File,
    log: &Path,
    timeout: Duration,
    try_acquire: fn(&File) -> std::result::Result<(), TryLockError>,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match try_acquire(file) {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(anyhow!(
                    "timed out after {timeout:?} waiting for the span log lock on {}; \
                     another aoa hook is holding it and may be wedged",
                    log.display()
                ))
            }
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("failed to lock {}", log.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A span of the given type and sequence, with the fields the lock tests
    /// never vary. They assert on ordering and on whether a write landed at
    /// all, so `source` and `attributes` are noise in every one of them.
    fn span(span_type: SpanType, seq: u64) -> Span {
        Span {
            span_type,
            source: SpanSource::Native,
            seq,
            attributes: Map::new(),
        }
    }

    #[test]
    fn sanitize_session_strips_path_traversal() {
        assert_eq!(sanitize_session("../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitize_session("a/b\\c"), "a-b-c");
        assert_eq!(sanitize_session("ok_id-9"), "ok_id-9");
        assert_eq!(sanitize_session("///"), "unknown");
        assert_eq!(sanitize_session(""), "unknown");
    }

    #[test]
    fn live_log_path_stays_inside_traces_dir() {
        let repo = tempfile::tempdir().unwrap();
        let path = live_log_path(repo.path(), "../escape");
        assert_eq!(path, repo.path().join(".aoa/traces/live-escape.jsonl"));
    }

    #[test]
    fn live_log_path_uses_the_pre_resolved_trust_root() {
        let trusted_repo = tempfile::tempdir().unwrap();
        let path = live_log_path(trusted_repo.path(), "sess-1");

        assert_eq!(
            path,
            trusted_repo.path().join(".aoa/traces/live-sess-1.jsonl")
        );
    }

    /// A hook must not inherit another process's stall; see
    /// [`lock_exclusive_bounded`] for why an unbounded wait is unsafe here.
    ///
    /// Uses a short timeout rather than [`LOCK_TIMEOUT`] to keep the test fast;
    /// the bounding behaviour is what is under test, not the value.
    #[test]
    fn a_held_lock_fails_the_waiter_instead_of_hanging_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_path(&dir, "live-held.jsonl");

        // A separate open file description, which is what the lock arbitrates on.
        let holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        holder.lock().unwrap();

        let waiter = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        let timeout = Duration::from_millis(50);
        let started = Instant::now();
        let err = lock_exclusive_bounded(&waiter, &log, timeout).unwrap_err();
        let waited = started.elapsed();

        assert!(
            err.to_string().contains("timed out"),
            "must report the timeout, got: {err}"
        );
        assert!(
            waited >= timeout,
            "must wait through the timeout and then give up, waited {waited:?}"
        );

        // Releasing the holder lets the next acquisition through, so the failure
        // is transient contention rather than a permanently poisoned log.
        drop(holder);
        lock_exclusive_bounded(&waiter, &log, LOCK_TIMEOUT)
            .expect("lock is available once released");
    }

    /// The append path itself must acquire the lock boundedly, not merely the
    /// helper that does the acquiring.
    ///
    /// Covering [`lock_exclusive_bounded`] alone leaves the wiring untested:
    /// replacing the call in [`append_span_within`] with a blocking acquisition
    /// reintroduces the session-freeze hazard while every other test still
    /// passes. Verified by exactly that mutation.
    #[test]
    fn a_contended_append_fails_bounded_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_path(&dir, "live-contended.jsonl");

        let holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        holder.lock().unwrap();

        let timeout = Duration::from_millis(50);
        // Run the append off-thread and wait with a deadline, so a regression to
        // a blocking acquisition surfaces as a diagnosable failure here rather
        // than hanging the whole test binary until CI times out.
        let (tx, rx) = std::sync::mpsc::channel();
        let target = log.clone();
        std::thread::spawn(move || {
            let outcome =
                append_span_within(&target, timeout, |seq| span(SpanType::WriteCommitted, seq));
            let _ = tx.send(outcome);
        });

        let err = rx
            .recv_timeout(timeout * 20)
            .expect("append never returned — it inherited the holder's stall instead of giving up")
            .expect_err("append must fail while the lock is held");

        assert!(
            err.to_string().contains("timed out"),
            "must report the timeout, got: {err}"
        );
        // Nothing was written: failing to lock must never fall through to an
        // unlocked append, which would reintroduce the corrupt seq. The holder
        // is released first because the reader takes the same lock (aoa-wew0);
        // asserting while it is still held would time out instead of inspecting
        // the file.
        drop(holder);
        assert!(
            read_spans(&log).unwrap().is_empty(),
            "a failed acquisition must not append"
        );
    }

    /// A reader must never observe a half-written final line.
    ///
    /// Claude Code dispatches parallel tool calls, so two hook subprocesses
    /// genuinely run at once and the R7 gate's read races an append. Before
    /// aoa-wew0 that read took no lock: it could parse a torn line, which fails
    /// the hook on input that is merely *in flight*. Tolerating the fragment
    /// instead would be worse — [`reproduction_gate`] blocks when no `test.run`
    /// is present, so dropping the very span being written turns a loud error
    /// into a silent, wrong `ReproductionRequired` block.
    #[test]
    fn a_reader_waits_for_an_in_flight_append_instead_of_tearing_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-inflight.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();

        // An appender caught mid-write: holds the exclusive lock, and only part
        // of its line has reached the file.
        let mut writer = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        writer.lock().unwrap();
        let line = serde_json::to_string(&span(SpanType::TestRun, 0)).unwrap();
        let (head, tail) = line.split_at(line.len() / 2);
        writer.write_all(head.as_bytes()).unwrap();

        let timeout = Duration::from_millis(50);
        let err = read_spans_within(&log, timeout)
            .expect_err("the reader must refuse the torn state, not parse it");
        assert!(
            err.to_string().contains("timed out"),
            "must report waiting for the writer, got: {err}"
        );

        // The appender finishes and releases; the committed state reads cleanly.
        writer.write_all(tail.as_bytes()).unwrap();
        writer.write_all(b"\n").unwrap();
        drop(writer);

        let spans = read_spans_within(&log, timeout).expect("committed state reads cleanly");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].span_type, SpanType::TestRun);
    }

    /// Also the regression guard for the writer deadlocking against itself.
    ///
    /// `read_spans` locks, and [`append_span_within`] derives `seq` by reading
    /// the log inside its own exclusive critical section. `flock` is held per
    /// *open file description*, so deriving that count through a second
    /// descriptor would make every append contend with the lock it already
    /// holds and die at [`LOCK_TIMEOUT`]. The appends below must simply succeed.
    ///
    /// Driven through the bounded variants so that regression fails in
    /// milliseconds rather than after the full production timeout — a guard that
    /// takes 5s per append to report is one people stop running. The unbounded
    /// [`append_span`] and [`read_spans`] wrappers are covered by
    /// `concurrent_appends_assign_distinct_sequence_numbers` and
    /// `read_spans_missing_file_is_empty_not_error`.
    #[test]
    fn append_then_read_round_trips_spans_monotonically() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(".aoa/traces/live-x.jsonl");
        let timeout = Duration::from_millis(50);
        for span_type in [SpanType::TestRun, SpanType::WriteAttempt] {
            append_span_within(&log, timeout, |seq| span(span_type, seq))
                .expect("an uncontended append must not wait on itself");
        }
        let spans = read_spans_within(&log, timeout).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].seq, 0);
        assert_eq!(spans[1].seq, 1);
        assert_eq!(spans[0].span_type, SpanType::TestRun);
    }

    #[test]
    fn read_spans_missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let spans = read_spans(&dir.path().join("nope.jsonl")).unwrap();
        assert!(spans.is_empty());
    }

    /// Concurrent appenders must produce one distinct `seq` each.
    ///
    /// Asserting only that the result *validates* would pass vacuously:
    /// `validate_trace` rejects a decreasing `seq` but accepts a repeated one,
    /// so the duplicate half of the race is invisible to it. The real assertion
    /// is that N appends yield exactly the set `0..N`.
    ///
    /// Every racer goes through `append_span`, which opens its own descriptor —
    /// `flock` is held per open file description, so a test sharing one handle
    /// would serialize for the wrong reason and prove nothing.
    #[test]
    fn concurrent_appends_assign_distinct_sequence_numbers() {
        const RACERS: u64 = 20;
        let dir = tempfile::tempdir().unwrap();
        let log = log_path(&dir, "live-race.jsonl");

        std::thread::scope(|scope| {
            for _ in 0..RACERS {
                scope.spawn(|| append_span(&log, SpanType::WriteCommitted, Map::new()).unwrap());
            }
        });

        let mut seqs: Vec<u64> = read_spans(&log).unwrap().iter().map(|s| s.seq).collect();
        assert_eq!(seqs.len(), RACERS as usize, "every append must land");
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (0..RACERS).collect::<Vec<_>>(),
            "seqs must be distinct and gapless; duplicates or gaps mean the \
             read-modify-write raced"
        );
    }

    /// A live-log path under `dir`, with its parent created.
    fn log_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        let log = dir.path().join(".aoa/traces").join(name);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        log
    }

    #[cfg(unix)]
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
    #[cfg(unix)]
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

    #[cfg(unix)]
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

    /// Write `raw` as the whole log, bypassing the append path, so a test can
    /// stage a log shape the append path would never produce.
    fn seed_log(dir: &tempfile::TempDir, raw: &str) -> PathBuf {
        let log = log_path(dir, "live-seeded.jsonl");
        std::fs::write(&log, raw).unwrap();
        log
    }

    /// One serialized span line, newline included.
    fn span_line(seq: u64) -> String {
        span_line_with(seq, Map::new())
    }

    /// [`span_line`], carrying `attributes` — which is how a fixture controls
    /// the line's length.
    fn span_line_with(seq: u64, attributes: Map<String, Value>) -> String {
        let span = Span {
            span_type: SpanType::TestRun,
            source: SpanSource::Native,
            seq,
            attributes,
        };
        format!("{}\n", serde_json::to_string(&span).unwrap())
    }

    /// The seq of the span the append path just wrote — i.e. the log's last.
    ///
    /// Reads the log in full, so it is only usable where every line parses. A
    /// test staging a deliberately corrupt line must read the tail by hand.
    fn last_seq(log: &Path) -> u64 {
        read_spans(log).unwrap().last().unwrap().seq
    }

    /// One span line whose serialized form exceeds `bytes`: the padding alone
    /// is that long, and it rides in an attribute so the line is a real span
    /// rather than filler.
    fn oversized_span_line(seq: u64, bytes: u64) -> String {
        let mut attributes = Map::new();
        attributes.insert(
            "path".to_string(),
            Value::String("p".repeat(bytes as usize)),
        );
        span_line_with(seq, attributes)
    }

    /// `seq` continues from the last one written, not from the number of spans.
    ///
    /// The two agree only on a gapless log. They diverge on one carrying a gap
    /// — which the pre-lock races (see [`append_span_with`]) actually produced —
    /// and there the count is the wrong answer: it re-emits a seq already in the
    /// file, and `validate_trace` compares adjacent pairs, so the resulting
    /// decrease wedges `aoa audit` for the whole repo with no repair path.
    #[test]
    fn append_continues_from_the_last_seq_not_the_span_count() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, &format!("{}{}", span_line(0), span_line(7)));
        append_span(&log, SpanType::WriteCommitted, Map::new()).unwrap();
        assert_eq!(
            last_seq(&log),
            8,
            "must be last+1; 2 would mean the count was used and the seq decreased"
        );
    }

    /// The regression guard for the bounded read itself (aoa-1lq0).
    ///
    /// Deriving `seq` must touch only the log's tail, so a log whose *earlier*
    /// lines are unparseable still appends. Any implementation that *validates
    /// every line* fails here — which is the point: every other test in this
    /// group passes under a full-file read, so without this one nothing pins
    /// the tail-only contract at all.
    ///
    /// It pins parsing, not I/O: an implementation that read the whole file into
    /// memory and still parsed only the last line would pass. The bound on how
    /// much is *read* is pinned by
    /// [`next_seq_refuses_to_widen_past_the_cap`] instead.
    ///
    /// This deliberately narrows corruption detection on the append path.
    /// Mid-log corruption is still caught loudly by the full [`read_spans`] a
    /// gating host performs, and again by validation at corpus ingest; refusing
    /// to append here would instead freeze all further recording on a log with
    /// any historical bad line — on an append-only file with no repair path.
    #[test]
    fn append_reads_only_the_tail_and_ignores_an_unparseable_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, &format!("not json at all\n{}", span_line(4)));
        append_span(&log, SpanType::WriteCommitted, Map::new()).unwrap();

        let raw = std::fs::read_to_string(&log).unwrap();
        let appended: Span = serde_json::from_str(raw.lines().last().unwrap()).unwrap();
        assert_eq!(appended.seq, 5);
    }

    /// A final line larger than one tail window must widen the read, not get
    /// truncated into a parse error.
    ///
    /// Ordinary lines precede the big one deliberately. A log consisting of
    /// *only* the oversized line would let the widen loop terminate by reaching
    /// the start of the file, leaving the case it actually has to get right —
    /// locating the newline that ends the line before it — unexercised.
    #[test]
    fn append_resolves_a_final_line_longer_than_one_tail_window() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(
            &dir,
            &format!(
                "{}{}{}",
                span_line(0),
                span_line(1),
                oversized_span_line(2, SEQ_TAIL_WINDOW * 3)
            ),
        );
        append_span(&log, SpanType::TestRun, Map::new()).unwrap();
        assert_eq!(last_seq(&log), 3);
    }

    /// Widening is bounded. Past the cap the read gives up loudly rather than
    /// growing without limit — which would surrender the very bound `next_seq`
    /// exists to establish.
    ///
    /// The cap is a parameter for the same reason the lock timeout is: the real
    /// 16 MiB [`MAX_SEQ_TAIL_BYTES`] would make this test write 16 MiB to prove
    /// a branch that is independent of the number.
    ///
    /// The numbers are chosen so an unclamped widen is caught as a wrong
    /// *answer*, not merely as an oversized read a test cannot observe. `cap` is
    /// deliberately not a power-of-two multiple of [`SEQ_TAIL_WINDOW`], and the
    /// log is shorter than the next doubling past it. A widen that clamps only
    /// against the log's length therefore reaches `start == 0`, parses the whole
    /// file as one line, and *succeeds* — so this assertion fails. Only a widen
    /// that also clamps against `cap` stops short and reports the cap.
    ///
    /// With the shipped constants the bug is dormant (16 MiB is an exact
    /// power-of-two multiple of 8 KiB, so doubling lands on the cap), which is
    /// precisely why the alignment must not be assumed here.
    #[test]
    fn next_seq_refuses_to_widen_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cap = SEQ_TAIL_WINDOW * 3;
        let log = seed_log(&dir, &oversized_span_line(1, cap + 1000));

        let len = std::fs::metadata(&log).unwrap().len();
        assert!(len > cap, "the log must exceed the cap, got {len} vs {cap}");
        assert!(
            len < SEQ_TAIL_WINDOW * 4,
            "and must be short enough that an unclamped widen would reach the \
             start of the file and wrongly succeed, got {len}"
        );

        let mut file = std::fs::OpenOptions::new().read(true).open(&log).unwrap();
        let err = next_seq(&mut file, &log, cap).unwrap_err();
        assert!(
            err.to_string().contains("exceeds"),
            "must name the exceeded tail read, got: {err}"
        );
    }

    /// The append path must never write a line it could not later read back.
    ///
    /// `next_seq` widens to at most [`MAX_SEQ_TAIL_BYTES`], so a longer line
    /// would append once and then refuse every subsequent append for the rest
    /// of the session — the log poisoning itself with a line it wrote. Reachable
    /// without any external corruption: `write_target` takes `file_path`
    /// straight from the hook payload, and nothing bounds its length.
    #[test]
    fn append_refuses_a_span_line_it_could_not_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_path(&dir, "live-huge.jsonl");
        let mut attributes = Map::new();
        attributes.insert(
            "path".to_string(),
            Value::String("p".repeat(MAX_SEQ_TAIL_BYTES as usize + 1)),
        );

        let err = append_span(&log, SpanType::WriteCommitted, attributes).unwrap_err();
        assert!(
            err.to_string().contains("refusing to append"),
            "must refuse before writing, got: {err}"
        );
        assert!(
            !log.exists() || std::fs::metadata(&log).unwrap().len() == 0,
            "the unreadable line must not have landed"
        );
    }

    /// `seq` must never wrap. A release build's `u64::MAX + 1` is `0`, which
    /// appends a *decreasing* `seq` — the corruption the whole locked
    /// read-modify-write exists to prevent, on a log with no repair path.
    #[test]
    fn append_refuses_to_wrap_the_sequence_number() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, &span_line(u64::MAX));

        let err = append_span(&log, SpanType::TestRun, Map::new()).unwrap_err();
        assert!(
            err.to_string().contains("wrap to zero"),
            "must name the exhaustion, got: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            span_line(u64::MAX),
            "refusing must leave the log byte-identical"
        );
    }

    /// A log of nothing but blank lines is empty in every sense `read_spans`
    /// recognizes, so it starts at zero rather than failing on an empty parse.
    #[test]
    fn an_entirely_blank_log_starts_at_seq_zero() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, "\n   \n\n");
        append_span(&log, SpanType::TestRun, Map::new()).unwrap();
        assert_eq!(last_seq(&log), 0);
    }

    /// Blank trailing lines are not corruption — [`read_spans`] skips them, and
    /// the tail scan must skip them too rather than handing an empty segment to
    /// the parser.
    #[test]
    fn append_skips_blank_trailing_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, &format!("{}\n   \n", span_line(3)));
        append_span(&log, SpanType::TestRun, Map::new()).unwrap();
        assert_eq!(last_seq(&log), 4);
    }

    /// A killed writer can leave a partial final line after a committed prefix.
    /// The next writer owns the exclusive lock needed to repair that tail:
    /// truncate to the last newline, then continue sequence numbering from the
    /// final complete span.
    #[test]
    fn append_repairs_a_log_whose_final_line_is_torn() {
        let dir = tempfile::tempdir().unwrap();
        let committed = span_line(0);
        let torn = format!("{committed}{{\"type\":\"test.run\"");
        let log = seed_log(&dir, &torn);

        append_span(&log, SpanType::TestRun, Map::new()).unwrap();
        let repaired = std::fs::read_to_string(&log).unwrap();
        assert!(repaired.starts_with(&committed));
        assert!(!repaired.contains(r#"{"type":"test.run"{"#));
        assert_eq!(last_seq(&log), 1);
    }

    /// The repair is returned, not printed, so the byte count a host renders has
    /// to be the count actually discarded. Asserting only that the file was
    /// repaired (as the two tests around this one do) would pass just as well
    /// against a hard-coded zero, leaving the operator a truncation notice that
    /// understates what was lost.
    #[test]
    fn a_repairing_append_reports_the_bytes_it_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let committed = span_line(0);
        let fragment = r#"{"type":"test.run""#;
        let log = seed_log(&dir, &format!("{committed}{fragment}"));

        let repair = append_span(&log, SpanType::TestRun, Map::new()).unwrap();
        assert_eq!(
            repair,
            Some(TornTailRepair {
                discarded_bytes: fragment.len() as u64
            })
        );
    }

    /// The common case reports nothing, so a host has no repair to render on an
    /// ordinary append. Without this, a returned `Some` on every append would
    /// still satisfy the test above and cry truncation on a healthy log.
    #[test]
    fn an_append_to_an_intact_log_reports_no_repair() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, &span_line(0));

        assert_eq!(
            append_span(&log, SpanType::TestRun, Map::new()).unwrap(),
            None
        );
        // The empty log is the other healthy shape: nothing to tear yet.
        let fresh = log_path(&dir, "live-fresh.jsonl");
        assert_eq!(
            append_span(&fresh, SpanType::TestRun, Map::new()).unwrap(),
            None
        );
    }

    #[test]
    fn append_repairs_a_fragment_with_no_committed_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let log = seed_log(&dir, r#"{"type":"test.run""#);

        append_span(&log, SpanType::TestRun, Map::new()).unwrap();
        let spans = read_spans(&log).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].seq, 0);
    }
}
