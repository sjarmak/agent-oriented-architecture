use super::*;

// R7 keystone end-to-end: the reproduction-before-mutation gate blocks a write
// when no test ran, and allows it once a reproduction span is recorded.
//
// These exercise stdin, so they use `assert_cmd::Command` (which has
// `write_stdin`) rather than the `std::process::Command` helper above.
pub(super) fn aoa_stdin() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("aoa").expect("aoa binary builds")
}

/// Run `aoa observe --repo <repo> --enforce`, asserting success.
pub(super) fn observe_enforce(repo: &Path) {
    aoa_stdin()
        .args(["observe", "--repo", repo.to_str().unwrap(), "--enforce"])
        .assert()
        .success();
}

pub(super) fn hook_payload(tool: &str, command: Option<&str>, cwd: &Path) -> String {
    std::fs::create_dir_all(cwd.join(".git")).expect("mark hook fixture as a repository");
    let mut input = serde_json::Map::new();
    match command {
        Some(c) => {
            input.insert("command".into(), Value::String(c.into()));
        }
        None => {
            input.insert("file_path".into(), Value::String("src/lib.rs".into()));
        }
    }
    serde_json::to_string(&serde_json::json!({
        "session_id": "it-session",
        "tool_name": tool,
        "tool_input": input,
        "cwd": cwd.to_str().unwrap(),
    }))
    .unwrap()
}

/// The live log the enforce hooks append to for `hook_payload`'s session.
fn live_log_path(repo: &Path) -> std::path::PathBuf {
    repo.join(".aoa/traces/live-it-session.jsonl")
}

fn live_log(repo: &Path) -> String {
    std::fs::read_to_string(live_log_path(repo)).expect("hooks created a live log")
}

/// The acceptance criterion, driven through the real CLI: each of the four
/// non-landing outcomes is recorded and observable, and none of them is the
/// span that edit ground truth is derived from.
///
/// Every outcome is a separate hook subcommand because the host raises a
/// separate event for each. Nothing here inspects a tool response.
#[test]
fn enforce_records_each_write_outcome_under_its_own_span() {
    for (subcommand, expected) in [
        ("commit", "write.committed"),
        ("fail", "write.failed"),
        ("deny", "write.denied"),
    ] {
        let repo = TempDir::new().unwrap();
        aoa_stdin()
            .args(["enforce", subcommand])
            .write_stdin(hook_payload("Edit", None, repo.path()))
            .assert()
            .success();

        let contents = live_log(repo.path());
        assert!(
            contents.contains(&format!(r#""type":"{expected}""#)),
            "`enforce {subcommand}` must record {expected}: {contents}"
        );
        assert!(
            contents.contains("src/lib.rs"),
            "{expected} carries its target path: {contents}"
        );
    }
}

/// An outcome hook reports history. It must never fail the tool call after the
/// fact, and it must ignore a tool it does not guard even if a stale or
/// hand-edited settings.json routes one to it.
#[test]
fn enforce_outcome_hooks_never_block_and_ignore_unguarded_tools() {
    let repo = TempDir::new().unwrap();
    aoa_stdin()
        .args(["enforce", "commit"])
        .write_stdin(hook_payload("Bash", Some("ls"), repo.path()))
        .assert()
        .success();

    assert!(
        !live_log_path(repo.path()).exists(),
        "a non-mutation tool records no write outcome"
    );
}

/// The full lifecycle of one edit: the gate allows it and logs intent, then the
/// host's success event lands the confirmation. Both spans coexist — the
/// attempt is kept for the intent-versus-outcome signal, while only the
/// committed span is ground truth.
#[test]
fn allowed_write_records_intent_then_confirmation() {
    let repo = TempDir::new().unwrap();
    // A reproduction first, so the R7 gate permits the write.
    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("pytest -q"), repo.path()))
        .assert()
        .success();
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Edit", None, repo.path()))
        .assert()
        .success();
    aoa_stdin()
        .args(["enforce", "commit"])
        .write_stdin(hook_payload("Edit", None, repo.path()))
        .assert()
        .success();

    let contents = live_log(repo.path());
    assert!(contents.contains(r#""type":"write.attempt""#), "{contents}");
    assert!(
        contents.contains(r#""type":"write.committed""#),
        "{contents}"
    );
    assert!(
        contents.find(r#""write.attempt""#) < contents.find(r#""write.committed""#),
        "intent is recorded before the outcome that settles it: {contents}"
    );
}

#[test]
fn enforce_check_blocks_write_without_reproduction() {
    let repo = TempDir::new().unwrap();
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked Write"));
}

/// A live log the gate cannot use must DENY the pending write, not let it
/// through. `run_check` has to read the log before it can decide at all, and
/// `block` has to append its `write.blocked` span before it can return exit 2,
/// so any I/O error on that path reaches the gate's own failure handling.
#[test]
fn enforce_check_fails_closed_when_the_span_log_is_unusable() {
    let repo = TempDir::new().unwrap();
    // A directory squatting the log path is unopenable as a file, standing in
    // for every other route to the same state (a FIFO, an unwritable file, a
    // lock that never frees).
    std::fs::create_dir_all(live_log_path(repo.path())).unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked"));
}

#[test]
fn enforce_writer_repairs_and_reports_an_unterminated_tail() {
    let repo = TempDir::new().unwrap();
    let log = live_log_path(repo.path());
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(
        &log,
        concat!(
            r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#,
            "\n",
            r#"{"type":"write.attempt""#
        ),
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload(
            "Bash",
            Some("cargo test --workspace"),
            repo.path(),
        ))
        .assert()
        .success()
        .stderr(
            predicate::str::contains("repaired").and(predicate::str::contains("unterminated tail")),
        );

    let spans: Vec<Value> = live_log(repo.path())
        .lines()
        .map(|line| serde_json::from_str(line).expect("every retained line is valid JSON"))
        .collect();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0]["seq"], 0);
    assert_eq!(spans[1]["seq"], 1);
}

/// A FIFO at the log path is the one squatter that does not simply error: `open`
/// blocks on it until a counterpart appears, so without an explicit file-type
/// check the hook hangs rather than failing, never reaches the fail-closed
/// conversion, and is eventually abandoned by the host — which is the same
/// permitted write the directory case used to produce.
///
/// The timeout is the assertion's teeth: a regression that drops the file-type
/// check fails here instead of wedging the test binary.
#[cfg(unix)]
#[test]
fn enforce_check_fails_closed_on_a_fifo_span_log() {
    let repo = TempDir::new().unwrap();
    let log = live_log_path(repo.path());
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&log)
        .status()
        .expect("mkfifo is available");
    assert!(made.success(), "mkfifo failed");

    aoa_stdin()
        .timeout(std::time::Duration::from_secs(20))
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked"));
}

#[cfg(unix)]
#[test]
fn enforce_check_refuses_a_symlinked_traces_directory() {
    let repo = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".aoa")).unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.path().join(".aoa/traces")).unwrap();

    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("cargo test"), repo.path()))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("symlink"));

    assert!(
        std::fs::read_dir(outside.path()).unwrap().next().is_none(),
        "the enforce lane must not create a live log through the directory symlink"
    );
}

/// The posture holds for the policy the gate is enforcing, not just for its log.
/// A policy it cannot parse is the case that most directly disables R5: the
/// protected-path check is the first thing `run_check` does with a policy, so an
/// unreadable one used to leave every protected path writable for the session.
#[test]
fn enforce_check_fails_closed_on_a_malformed_policy() {
    let repo = TempDir::new().unwrap();
    std::fs::write(repo.path().join("aoa-policy.yaml"), "protected_paths: [\n").unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked"));
}

/// The same posture one step earlier: a payload the gate cannot parse leaves it
/// unable to evaluate anything, so it denies rather than waving the write past.
#[test]
fn enforce_check_fails_closed_on_an_unparseable_payload() {
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin("{not json")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked"));
}

/// Fail-closed is scoped to the gate. The recording hooks report history and
/// have no write to deny, so an unusable log surfaces as a plain error (exit 1)
/// — never exit 2, which would let a bookkeeping failure block an edit the gate
/// already allowed.
///
/// Every recording subcommand is exercised, not just one: they are what the
/// posture decision in `denies_on_failure` classifies together, so a change that
/// swept one of them into the deny side would otherwise go unnoticed. The stderr
/// assertion pins the failure to the unusable log rather than to any path that
/// happens to return an error.
#[test]
fn enforce_recording_hooks_stay_fail_open_when_the_span_log_is_unusable() {
    for (subcommand, tool, command) in [
        ("record", "Bash", Some("cargo test --all")),
        ("commit", "Edit", None),
        ("fail", "Edit", None),
        ("deny", "Edit", None),
    ] {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir_all(live_log_path(repo.path())).unwrap();

        aoa_stdin()
            .args(["enforce", subcommand])
            .write_stdin(hook_payload(tool, command, repo.path()))
            .assert()
            .code(1)
            .stderr(predicate::str::contains("live-it-session.jsonl"));
    }
}

#[test]
fn enforce_allows_write_after_a_test_run_is_recorded() {
    let repo = TempDir::new().unwrap();

    // 1. Pre-reproduction write is blocked (exit 2).
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .code(2);

    // 2. A test command records a test.run span (PostToolUse, never blocks).
    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("cargo test --all"), repo.path()))
        .assert()
        .success();

    // 3. The same write now passes the gate (exit 0).
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .success();

    // The live log carries the test.run, the earlier write.blocked span, and
    // the allowed write recorded as write.attempt with its target path — the
    // held-out ground truth the live corpus accumulates (aoa-d6t.23).
    let contents = live_log(repo.path());
    assert!(contents.contains("test.run"));
    assert!(contents.contains("write.blocked"));
    assert!(
        contents.contains(r#""type":"write.attempt""#),
        "allowed write must land as write.attempt: {contents}"
    );
    assert!(
        contents.contains(r#""path":"src/lib.rs""#),
        "write.attempt carries its target path: {contents}"
    );
}

// An allowed write is recorded even when policy disables the reproduction
// gate: held-out truth capture is independent of gating (aoa-d6t.23).
#[test]
fn enforce_check_records_allowed_write_when_reproduction_is_disabled() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n",
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Edit", None, repo.path()))
        .assert()
        .success();

    let contents = live_log(repo.path());
    assert!(
        contents.contains(r#""type":"write.attempt""#) && contents.contains("src/lib.rs"),
        "allowed write recorded with its path: {contents}"
    );
}

#[test]
fn enforce_check_blocks_protected_path_even_without_reproduction() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\".github/**\"]\nreproduction_required: false\n",
    )
    .unwrap();

    let mut input = serde_json::Map::new();
    input.insert(
        "file_path".into(),
        Value::String(".github/workflows/ci.yml".into()),
    );
    let payload = serde_json::to_string(&serde_json::json!({
        "session_id": "it-prot",
        "tool_name": "Write",
        "tool_input": input,
        "cwd": repo.path().to_str().unwrap(),
    }))
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(payload)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("protected path"));
}

#[test]
fn enforce_reproduction_toggle_off_allows_unprotected_write() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\".github/**\"]\nreproduction_required: false\n",
    )
    .unwrap();
    // Unprotected path, gate disabled by policy -> allowed with no test.run.
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Write", None, repo.path()))
        .assert()
        .success();
}

#[test]
fn enforce_check_rejects_an_arbitrary_non_repository_cwd_without_writing() {
    let dir = TempDir::new().unwrap();
    let payload = serde_json::to_string(&serde_json::json!({
        "session_id": "it-untrusted-root",
        "tool_name": "Write",
        "tool_input": {"file_path": "src/lib.rs"},
        "cwd": dir.path().to_str().unwrap(),
    }))
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(payload)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not inside a Git repository"));
    assert!(
        !dir.path().join(".aoa").exists(),
        "a rejected payload must not create telemetry outside a repository"
    );
}

/// A hook payload writing to an explicit `file_path` (the generated/protected
/// path tests need a target other than the default `src/lib.rs`).
fn write_payload(file_path: &str, session: &str, cwd: &Path) -> String {
    std::fs::create_dir_all(cwd.join(".git")).expect("mark hook fixture as a repository");
    let mut input = serde_json::Map::new();
    input.insert("file_path".into(), Value::String(file_path.into()));
    serde_json::to_string(&serde_json::json!({
        "session_id": session,
        "tool_name": "Write",
        "tool_input": input,
        "cwd": cwd.to_str().unwrap(),
    }))
    .unwrap()
}

#[test]
fn enforce_check_blocks_write_to_declared_generated_path() {
    let repo = TempDir::new().unwrap();
    // Gate off so only the R6 generated-artifact block can fire — isolates it.
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n\
         generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n",
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_payload(
            "crates/api/types.gen.rs",
            "it-gen",
            repo.path(),
        ))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("generated artifact"))
        .stderr(predicate::str::contains("schema.json"));

    // The write.blocked span records the source as its own machine-readable attr.
    let log = repo.path().join(".aoa/traces/live-it-gen.jsonl");
    let contents = std::fs::read_to_string(&log).expect("live log written");
    assert!(contents.contains("write.blocked"));
    assert!(contents.contains("generated_artifact"));
    assert!(contents.contains("\"source\":\"schema.json\""));
}

#[test]
fn enforce_check_blocks_write_to_bare_glob_generated_path() {
    // Back-compat form: a bare-string generated_paths entry (no `source:`). The
    // block must still fire; the redirect falls back to the glob itself.
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n\
         generated_paths:\n  - \"**/*.gen.rs\"\n",
    )
    .unwrap();

    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_payload(
            "crates/api/types.gen.rs",
            "it-bare",
            repo.path(),
        ))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("generated artifact"))
        // No declared source -> the redirect names the glob itself.
        .stderr(predicate::str::contains("**/*.gen.rs"));

    let log = repo.path().join(".aoa/traces/live-it-bare.jsonl");
    let contents = std::fs::read_to_string(&log).expect("live log written");
    assert!(contents.contains("write.blocked"));
    assert!(contents.contains("\"source\":\"**/*.gen.rs\""));
}

#[test]
fn enforce_check_allows_write_to_non_generated_path() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "reproduction_required: false\n\
         generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n",
    )
    .unwrap();
    // A hand-written source file is not generated -> allowed.
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(write_payload(
            "crates/api/handler.rs",
            "it-gen-ok",
            repo.path(),
        ))
        .assert()
        .success();
}
