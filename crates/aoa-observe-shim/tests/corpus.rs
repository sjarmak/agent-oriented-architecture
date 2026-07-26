//! Acceptance: observe-captured runs accumulate into a parseable trace corpus
//! (aoa-d6t.23, piece 2).

use std::path::Path;

use aoa_observe_shim::{held_out_edits, load_corpus, ObserveShimError};
use tempfile::TempDir;

const SPAN_TEST_RUN: &str = r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#;
/// Intent, logged before the tool runs. On its own it proves nothing landed.
const SPAN_WRITE: &str =
    r#"{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"src/lib.rs"}}"#;
/// The post-execution success that makes the edit above ground truth.
const SPAN_COMMITTED: &str =
    r#"{"type":"write.committed","source":"native","seq":2,"attributes":{"path":"src/lib.rs"}}"#;
const SPAN_BLOCKED: &str =
    r#"{"type":"write.blocked","source":"native","seq":3,"attributes":{"path":"deny.rs"}}"#;
const SPAN_FAILED: &str =
    r#"{"type":"write.failed","source":"native","seq":4,"attributes":{"path":"boom.rs"}}"#;
const SPAN_DENIED: &str =
    r#"{"type":"write.denied","source":"native","seq":5,"attributes":{"path":"refused.rs"}}"#;

fn traces_dir(repo: &Path) -> std::path::PathBuf {
    let dir = repo.join(".aoa").join("traces");
    std::fs::create_dir_all(&dir).expect("create traces dir");
    dir
}

fn write_live_log(repo: &Path, session: &str, lines: &[&str]) {
    let path = traces_dir(repo).join(format!("live-{session}.jsonl"));
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("write live log");
}

#[test]
fn never_observed_repo_yields_an_empty_corpus_not_an_error() {
    let repo = TempDir::new().expect("tempdir");
    let corpus = load_corpus(repo.path()).expect("no traces dir is a defined state");
    assert_eq!(corpus.observations(), 0);
    assert!(corpus.sessions.is_empty());
    assert!(corpus.skipped.is_empty());
}

#[test]
fn only_sessions_carrying_held_out_edits_count_as_observations() {
    let repo = TempDir::new().expect("tempdir");
    // Two live sessions from the enforce hooks: one landed an edit, one only
    // ran tests...
    write_live_log(
        repo.path(),
        "s1",
        &[SPAN_TEST_RUN, SPAN_WRITE, SPAN_COMMITTED],
    );
    write_live_log(repo.path(), "s2", &[SPAN_TEST_RUN]);
    // ...plus one whole trace landed via the write_trace path, edit-free.
    let trace_json = format!(r#"{{"spans":[{SPAN_TEST_RUN}]}}"#);
    std::fs::write(traces_dir(repo.path()).join("run-1.json"), trace_json).expect("write trace");

    let corpus = load_corpus(repo.path()).expect("parseable corpus");
    assert_eq!(corpus.sessions.len(), 3, "every session is ingested");
    assert_eq!(
        corpus.observations(),
        1,
        "only the session with a landed edit supplies held-out signal"
    );

    let ids: Vec<&str> = corpus
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["s1", "s2", "run-1"], "deterministic name order");

    // The live-session trace carries the dev's real edit as held-out truth.
    let s1 = &corpus.sessions[0];
    assert_eq!(s1.trace.spans.len(), 3);
    let edits = held_out_edits(&s1.trace);
    assert_eq!(edits.into_iter().collect::<Vec<_>>(), vec!["src/lib.rs"]);
}

// The precondition measures available behavioral signal, not session-file
// count: a zero-byte live log parses to an empty trace and must contribute
// no observation (aoa-d6t.23 review finding).
#[test]
fn contentless_live_logs_supply_zero_observations() {
    let repo = TempDir::new().expect("tempdir");
    for i in 0..10 {
        std::fs::write(traces_dir(repo.path()).join(format!("live-s{i}.jsonl")), "")
            .expect("write empty log");
    }

    let corpus = load_corpus(repo.path()).expect("parseable corpus");
    assert_eq!(corpus.sessions.len(), 10);
    assert_eq!(
        corpus.observations(),
        0,
        "zero-span sessions are not held-out behavioral signal"
    );
}

/// The acceptance criterion in one test: of the write outcomes a session can
/// record, exactly one is ground truth. The other four stay in the trace and
/// stay observable, but none of them may be counted as an edit.
#[test]
fn held_out_edits_admit_only_confirmed_successful_mutations() {
    let repo = TempDir::new().expect("tempdir");
    write_live_log(
        repo.path(),
        "s",
        &[
            SPAN_TEST_RUN,
            SPAN_WRITE,
            SPAN_COMMITTED,
            SPAN_BLOCKED,
            SPAN_FAILED,
            SPAN_DENIED,
        ],
    );
    let corpus = load_corpus(repo.path()).expect("parseable");
    let trace = &corpus.sessions[0].trace;

    let edits = held_out_edits(trace);
    assert_eq!(
        edits.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["src/lib.rs"],
        "only the committed write is ground truth"
    );

    // Each non-landing outcome is named individually so a regression says which
    // one leaked rather than just that the set grew.
    for (path, outcome) in [
        ("deny.rs", "a write the policy gate blocked"),
        ("boom.rs", "a write that ran and errored"),
        ("refused.rs", "a write the host refused"),
    ] {
        assert!(
            !edits.contains(path),
            "{outcome} never landed; it is not ground truth"
        );
    }

    // ...and all six spans survive ingestion: excluded from ground truth is not
    // the same as discarded. The failure modes stay auditable.
    assert_eq!(trace.spans.len(), 6, "no span is dropped on ingest");
}

#[test]
fn non_corpus_entries_are_recorded_not_silently_ignored() {
    let repo = TempDir::new().expect("tempdir");
    write_live_log(
        repo.path(),
        "s",
        &[SPAN_TEST_RUN, SPAN_WRITE, SPAN_COMMITTED],
    );
    let dir = traces_dir(repo.path());
    std::fs::write(dir.join("notes.txt"), "not a trace").expect("write stray file");
    std::fs::create_dir(dir.join("subdir")).expect("create stray dir");

    let corpus = load_corpus(repo.path()).expect("parseable");
    assert_eq!(corpus.observations(), 1);
    assert_eq!(corpus.skipped.len(), 2, "stray entries are recorded");
}

#[cfg(unix)]
#[test]
fn symlinked_corpus_file_is_skipped() {
    use std::os::unix::fs::symlink;

    let repo = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside dir");
    let target = outside.path().join("real.jsonl");
    std::fs::write(&target, format!("{SPAN_TEST_RUN}\n")).expect("write target");
    symlink(&target, traces_dir(repo.path()).join("live-evil.jsonl")).expect("symlink");

    let corpus = load_corpus(repo.path()).expect("parseable");
    assert_eq!(
        corpus.observations(),
        0,
        "an out-of-tree symlink target must not count as signal"
    );
    assert_eq!(corpus.skipped.len(), 1);
}

#[test]
fn corrupt_live_log_fails_loud_naming_the_file() {
    let repo = TempDir::new().expect("tempdir");
    write_live_log(repo.path(), "bad", &[SPAN_TEST_RUN, "not json"]);

    let err = load_corpus(repo.path()).expect_err("corruption must not be skipped");
    let msg = err.to_string();
    assert!(
        matches!(err, ObserveShimError::Ingest { .. }),
        "expected Ingest, got {err:?}"
    );
    assert!(msg.contains("live-bad.jsonl"), "names the file: {msg}");
}

/// An audit run during an active session reads a log the enforce hooks are still
/// appending to, so its final line can be half written. That is an in-flight
/// record, not corruption: ingest must yield the committed spans before it
/// rather than failing the whole repo's corpus (aoa-wew0).
///
/// Exhaustive over every byte prefix rather than one hand-picked cut. The tear
/// that matters lands *inside* a multi-byte character, which fails UTF-8
/// decoding before any line-level check runs; the fixture's final line carries
/// an em dash because every R6 `write.blocked` span does (aoa-enforce's
/// `GeneratedArtifact` reason), so that case is covered by construction.
///
/// The complementary guarantee — a newline-terminated malformed line still fails
/// loud — is pinned by `corrupt_live_log_fails_loud_naming_the_file` above.
/// Together they say tolerance is about *framing*, never about content.
#[test]
fn a_partially_written_final_line_ingests_as_the_committed_prefix() {
    const IN_FLIGHT: &str = r#"{"type":"write.blocked","source":"native","seq":2,"attributes":{"path":"a.rs","reason":"generated artifact: 'a.rs' is derived from 'a.tmpl' — edit 'a.tmpl' and regenerate"}}"#;
    assert!(
        IN_FLIGHT.contains('—'),
        "the fixture must exercise a torn multi-byte character"
    );

    let committed = format!("{SPAN_TEST_RUN}\n{SPAN_WRITE}\n");
    let repo = TempDir::new().expect("tempdir");
    let path = traces_dir(repo.path()).join("live-inflight.jsonl");
    for cut in 1..IN_FLIGHT.len() {
        let mut bytes = committed.as_bytes().to_vec();
        bytes.extend_from_slice(&IN_FLIGHT.as_bytes()[..cut]);
        std::fs::write(&path, &bytes).expect("write in-flight live log");

        let corpus = load_corpus(repo.path())
            .unwrap_or_else(|err| panic!("a {cut}-byte in-flight tail must ingest: {err}"));
        assert_eq!(corpus.sessions.len(), 1, "cut {cut}");
        assert_eq!(
            corpus.sessions[0].trace.spans.len(),
            2,
            "the in-flight record must not count until it is framed (cut {cut})"
        );
    }
}

#[test]
fn out_of_order_trace_fails_validation() {
    let repo = TempDir::new().expect("tempdir");
    let reversed = r#"{"type":"test.run","source":"native","seq":5,"attributes":{}}
{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"a.rs"}}"#;
    std::fs::write(
        traces_dir(repo.path()).join("live-ooo.jsonl"),
        format!("{reversed}\n"),
    )
    .expect("write log");

    let err = load_corpus(repo.path()).expect_err("ordering is enforced at ingest");
    assert!(
        matches!(err, ObserveShimError::InvalidTrace { .. }),
        "expected InvalidTrace, got {err:?}"
    );
}

#[test]
fn malformed_json_trace_file_fails_schema() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(traces_dir(repo.path()).join("run.json"), "{").expect("write file");
    let err = load_corpus(repo.path()).expect_err("schema failure is loud");
    assert!(
        matches!(err, ObserveShimError::Schema { .. }),
        "expected Schema, got {err:?}"
    );
}

#[test]
fn json_trace_with_unsupported_version_is_rejected() {
    // The corpus ingest path — not `validate_trace` — is where codeprobe-produced
    // `.json` traces actually enter, so the wire-version guard must fire here.
    let repo = TempDir::new().expect("tempdir");
    let trace_json = format!(r#"{{"version":999,"spans":[{SPAN_TEST_RUN}]}}"#);
    std::fs::write(traces_dir(repo.path()).join("run.json"), trace_json).expect("write trace");

    let err = load_corpus(repo.path()).expect_err("version mismatch is loud");
    match err {
        ObserveShimError::InvalidTrace { source, .. } => assert!(
            matches!(source, aoa_trace::TraceError::UnsupportedVersion { .. }),
            "expected UnsupportedVersion, got {source:?}"
        ),
        other => panic!("expected InvalidTrace, got {other:?}"),
    }
}
