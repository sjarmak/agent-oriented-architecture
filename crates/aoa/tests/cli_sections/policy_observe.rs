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
    assert!(first.contains("aoa enforce check"));
    assert!(first.contains("aoa enforce record"));

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
        settings["aoa"]["enforce_hook_set_version"], 1,
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
