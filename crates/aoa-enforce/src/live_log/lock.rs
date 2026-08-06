//! Bounded waiting: taking the log's advisory lock with a deadline, so a hook
//! never inherits another process's stall.
//!
//! Both modes share one acquisition loop, so the bound is a property of the
//! lock rather than of whichever caller happens to take it.

use std::fs::{File, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

/// How long a hook waits for the span log's lock before failing. Generous
/// against real contention (the lock spans one read and one append) and short
/// enough that a wedged holder cannot stall the session.
pub(super) const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll interval while waiting. Short enough to be invisible under normal
/// contention, long enough not to spin a core against a wedged holder.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(5);

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
pub(super) fn lock_exclusive_bounded(file: &File, log: &Path, timeout: Duration) -> Result<()> {
    lock_bounded(file, log, timeout, File::try_lock)
}

/// [`lock_exclusive_bounded`] for a reader: takes the log's shared lock, so
/// concurrent readers do not exclude each other but none of them overlaps an
/// in-flight append. Bounded for the same reason the exclusive acquisition is —
/// a reader that inherits a wedged writer's stall freezes the session just as
/// thoroughly as a writer that does.
pub(super) fn lock_shared_bounded(file: &File, log: &Path, timeout: Duration) -> Result<()> {
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
    use super::super::test_support::log_path;
    use super::*;

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
}
