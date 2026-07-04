//! Acceptance: observe-captured runs accumulate into a parseable trace corpus
//! (aoa-d6t.23, piece 2).

use std::path::Path;

use aoa_observe_shim::{held_out_edits, load_corpus, ObserveShimError};
use tempfile::TempDir;

const SPAN_TEST_RUN: &str = r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#;
const SPAN_WRITE: &str =
    r#"{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"src/lib.rs"}}"#;
const SPAN_BLOCKED: &str =
    r#"{"type":"write.blocked","source":"native","seq":2,"attributes":{"path":"deny.rs"}}"#;

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
fn sessions_accumulate_one_observation_each() {
    let repo = TempDir::new().expect("tempdir");
    // Two live sessions from the enforce hooks...
    write_live_log(repo.path(), "s1", &[SPAN_TEST_RUN, SPAN_WRITE]);
    write_live_log(repo.path(), "s2", &[SPAN_TEST_RUN]);
    // ...plus one whole trace landed via the write_trace path.
    let trace_json = format!(r#"{{"spans":[{SPAN_TEST_RUN}]}}"#);
    std::fs::write(traces_dir(repo.path()).join("run-1.json"), trace_json).expect("write trace");

    let corpus = load_corpus(repo.path()).expect("parseable corpus");
    assert_eq!(corpus.observations(), 3, "each session is one observation");

    let ids: Vec<&str> = corpus
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["s1", "s2", "run-1"], "deterministic name order");

    // The live-session trace carries the dev's real edit as held-out truth.
    let s1 = &corpus.sessions[0];
    assert_eq!(s1.trace.spans.len(), 2);
    let edits = held_out_edits(&s1.trace);
    assert_eq!(edits.into_iter().collect::<Vec<_>>(), vec!["src/lib.rs"]);
}

#[test]
fn held_out_edits_exclude_blocked_writes() {
    let repo = TempDir::new().expect("tempdir");
    write_live_log(repo.path(), "s", &[SPAN_TEST_RUN, SPAN_WRITE, SPAN_BLOCKED]);
    let corpus = load_corpus(repo.path()).expect("parseable");
    let edits = held_out_edits(&corpus.sessions[0].trace);
    assert!(edits.contains("src/lib.rs"));
    assert!(
        !edits.contains("deny.rs"),
        "a denied write never landed; it is not ground truth"
    );
}

#[test]
fn non_corpus_entries_are_recorded_not_silently_ignored() {
    let repo = TempDir::new().expect("tempdir");
    write_live_log(repo.path(), "s", &[SPAN_TEST_RUN]);
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
