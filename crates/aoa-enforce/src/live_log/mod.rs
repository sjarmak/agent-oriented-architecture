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
//! One invariant per submodule: `open` holds containment, `lock` holds bounded
//! waiting, `seq` holds monotonicity and torn-tail repair. This file keeps the
//! [`LiveLog`] facade and the append/read orchestration that composes all three
//! — the all-or-nothing rollback lives with the write it guards.
//!
//! ## Trust root
//!
//! [`LiveLog::for_session`] takes the repository root as `base` and treats it as
//! already-resolved trust: it is opened directly, and only the `.aoa` and
//! `traces` components below it are acquired with `O_NOFOLLOW`. Resolving an
//! untrusted path *into* a repository root is the caller's job, deliberately not
//! this module's. The session id needs no such care — it is coerced to a single
//! safe filename component here.

mod lock;
mod open;
mod seq;
#[cfg(test)]
mod test_support;

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{Map, Value};

use aoa_trace::{validate_single_component, Span, SpanSource, SpanType};

use lock::{lock_exclusive_bounded, lock_shared_bounded, LOCK_TIMEOUT};
use open::{is_not_found, open_log, LogAccess};
use seq::{next_seq, parse_span_line, repair_torn_tail, MAX_SEQ_TAIL_BYTES};

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

/// Read the live log into spans, tolerating a missing file (no reproduction yet)
/// but failing loud on a corrupt line — a malformed log is a real defect, not
/// something to silently skip.
///
/// Taken under a bounded **shared** lock, so the read never overlaps an in-flight
/// [`append_span_with`]. Without it a reader could observe a half-written final
/// line, and both ways of handling that are wrong: parsing it fails the hook on
/// input that is merely in flight, while skipping it feeds [`reproduction_gate`](crate::reproduction_gate)
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
        open::create_traces_dir(parent)?;
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

#[cfg(test)]
mod tests {
    use super::test_support::{log_path, seed_log, span_line};
    use super::*;

    /// A span of the given type and sequence, with the fields the orchestration
    /// tests never vary. They assert on ordering and on whether a write landed
    /// at all, so `source` and `attributes` are noise in every one of them.
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
    /// instead would be worse — [`reproduction_gate`](crate::reproduction_gate)
    /// blocks when no `test.run` is present, so dropping the very span being
    /// written turns a loud error into a silent, wrong `ReproductionRequired`
    /// block.
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

    /// The repair is returned, not printed, so the byte count a host renders has
    /// to be the count actually discarded. Asserting only that the file was
    /// repaired (as the torn-tail tests beside [`repair_torn_tail`] do) would
    /// pass just as well against a hard-coded zero, leaving the operator a
    /// truncation notice that understates what was lost.
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
}
