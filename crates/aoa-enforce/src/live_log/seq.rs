//! Monotonic `seq`: deriving the next sequence number from the log's tail, and
//! repairing a torn tail before anything is spliced onto it.
//!
//! Everything here runs inside the writer's exclusive lock, against the very
//! handle that holds it — see [`super::append_span_within`] for why a second
//! descriptor would deadlock against that lock rather than merely cost an open.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use aoa_trace::Span;

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
pub(super) const MAX_SEQ_TAIL_BYTES: u64 = 16 * 1024 * 1024;

/// Remove an unterminated final fragment while holding the writer's exclusive
/// lock. A complete log is untouched; a file containing only a fragment is
/// reset to empty. The backward scan has no corpus-sized allocation and runs
/// only on the exceptional recovery path.
pub(super) fn repair_torn_tail(file: &mut File, log: &Path) -> Result<Option<u64>> {
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
/// and since that read happens inside [`super::append_span_within`]'s exclusive
/// lock, a long session's writes were both quadratic and serialized. Hook
/// latency lands directly in the agent's tool path, so that cost is
/// agent-visible.
///
/// *Last + 1*, not *count*, and the difference is correctness rather than
/// convenience. `validate_trace` compares **adjacent** spans, rejecting only a
/// `seq` lower than its predecessor's. On a log carrying a gap — which the
/// pre-lock races described on [`super::append_span_with`] really did produce —
/// the count is smaller than the last `seq`, so a count-derived append writes a
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
pub(super) fn next_seq(file: &mut File, log: &Path, max_tail: u64) -> Result<u64> {
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
/// Shared by [`super::read_spans`] and [`parse_seq`] so the two paths that read
/// this file report the same corruption identically. They used to spell the
/// context separately, which made the wording a convention rather than a fact:
/// adding a byte offset or a repair hint to one would silently leave the other
/// behind.
pub(super) fn parse_span_line(line: &[u8], log: &Path) -> Result<Span> {
    serde_json::from_slice(line).with_context(|| format!("corrupt span line in {}", log.display()))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{seed_log, span_line, span_line_with};
    use super::super::{append_span, read_spans};
    use super::*;
    use aoa_trace::SpanType;
    use serde_json::{Map, Value};

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
    /// — which the pre-lock races (see [`super::super::append_span_with`])
    /// actually produced — and there the count is the wrong answer: it re-emits
    /// a seq already in the file, and `validate_trace` compares adjacent pairs,
    /// so the resulting decrease wedges `aoa audit` for the whole repo with no
    /// repair path.
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
    /// Mid-log corruption is still caught loudly by the full
    /// [`super::super::read_spans`] a gating host performs, and again by
    /// validation at corpus ingest; refusing to append here would instead
    /// freeze all further recording on a log with any historical bad line — on
    /// an append-only file with no repair path.
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

    /// Blank trailing lines are not corruption — [`super::super::read_spans`]
    /// skips them, and the tail scan must skip them too rather than handing an
    /// empty segment to the parser.
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
