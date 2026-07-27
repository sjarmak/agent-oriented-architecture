use super::migrate::migrate_repo;
use super::*;

// --- aoa audit --self (aoa-d6t.19, leg 2: R14 lint-thyself) -------------------
// The toolkit measures its own added context tokens (the files its applied
// migration wrote, before vs after) and flags a regression when the median
// rose without demonstrated held-out gain.

#[test]
fn audit_self_without_migration_reports_absent_and_exits_zero() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no applied migration"));
}

#[test]
fn audit_self_flags_regression_when_context_rose_without_heldout_evidence() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a flagged regression exits non-zero"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["status"], "measured");
    assert_eq!(parsed["median_before_tokens"], 0.0);
    assert!(parsed["median_after_tokens"].as_f64().expect("median") > 0.0);
    assert_eq!(parsed["held_out"]["status"], "absent");
    assert_eq!(parsed["context_regression"], true);
}

#[test]
fn audit_self_human_names_the_regression() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("context regression"))
        .stdout(predicate::str::contains("held-out evidence: absent"));
}

#[test]
fn audit_self_with_heldout_gain_clears_the_regression() {
    // The fixture pair yields held_out_delta > 0 (label good in `eval compare`),
    // so the context rise is justified and no regression is flagged.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["held_out"]["status"], "present");
    assert!(
        parsed["held_out"]["held_out_delta"]
            .as_f64()
            .expect("delta")
            > 0.0
    );
    assert_eq!(parsed["context_regression"], false);
}

#[test]
fn audit_self_baseline_without_migrated_is_rejected() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .assert()
        .failure();
}

#[test]
fn audit_baseline_without_self_is_rejected() {
    // The held-out pair only means something to the self-audit; supplying it to
    // a plain audit is a usage error, not silently ignored.
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .assert()
        .failure();
}

/// Write a hand-crafted migration manifest under `repo`, as a corrupted or
/// hostile checkout could contain.
fn write_migration_manifest(repo: &Path, manifest: &Value) {
    std::fs::create_dir_all(repo.join(".aoa/migrate")).unwrap();
    std::fs::write(
        repo.join(".aoa/migrate/manifest.json"),
        manifest.to_string(),
    )
    .unwrap();
}

#[test]
fn audit_self_resolves_manifest_paths_against_repo_not_cwd() {
    // `aoa migrate --apply` with a relative `--repo` (the default is `.`)
    // records entry paths like `./README.md` verbatim. The self-audit must
    // resolve them against `--repo`, not the audit process's cwd — otherwise
    // it silently tokenizes an unrelated same-named file (fabricated counts)
    // or hard-errors on a perfectly intact migration record.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo", "."])
        .current_dir(repo.path())
        .assert()
        .success();

    // Audit from an unrelated cwd that contains a decoy README.md.
    let elsewhere = TempDir::new().expect("tempdir");
    std::fs::write(elsewhere.path().join("README.md"), "x").unwrap();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .current_dir(elsewhere.path())
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "no held-out evidence, so the rise is still a regression; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let readme = parsed["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|f| f["path"].as_str().expect("path").ends_with("README.md"))
        .expect("README.md entry");
    assert!(
        readme["after_tokens"].as_u64().expect("after_tokens") > 1,
        "must measure the migration-written README, not the 1-byte decoy: {readme}"
    );
}

#[test]
fn audit_self_escapes_manifest_text_in_human_output() {
    // Entry paths and fix ids come verbatim from on-disk manifest JSON — the
    // same trust level as falsification.json, whose fields the report escapes.
    // A crafted path or fix id must not put a raw ESC byte on the terminal.
    let repo = TempDir::new().expect("tempdir");
    let evil_name = "evil\u{1b}[2Jfile.md";
    std::fs::write(repo.path().join(evil_name), "hello world").unwrap();
    write_migration_manifest(
        repo.path(),
        &serde_json::json!({
            "fixes_applied": ["fix\u{1b}[31mred"],
            "entries": [{ "action": "created", "path": repo.path().join(evil_name) }],
        }),
    );

    let output = aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('\u{1b}'),
        "raw ESC reached the terminal: {stdout:?}"
    );
    assert!(
        stdout.contains("\\u{1b}"),
        "escaped form must be shown: {stdout:?}"
    );
}

#[test]
fn audit_self_rejects_manifest_entry_outside_the_repo() {
    // Apply validates every target stays inside the repo, so an entry that
    // resolves outside it is a corrupted (or hostile) migration record — a
    // hard error, never a read of arbitrary reachable files (/dev/zero,
    // /etc/passwd) on the manifest's say-so.
    let repo = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("tempdir");
    let secret = outside.path().join("secret.md");
    std::fs::write(&secret, "outside the audited repo").unwrap();
    write_migration_manifest(
        repo.path(),
        &serde_json::json!({
            "fixes_applied": ["r14-anchor"],
            "entries": [{ "action": "created", "path": secret }],
        }),
    );

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("outside the repo"));
}

#[test]
fn audit_self_caps_migration_written_file_reads() {
    // Migration-written files are read through the same byte cap as every
    // other untrusted file the CLI touches; a pathological entry cannot
    // exhaust memory.
    let repo = TempDir::new().expect("tempdir");
    let big = repo.path().join("big.md");
    std::fs::write(&big, vec![b'x'; 16 * 1024 * 1024 + 1]).unwrap();
    write_migration_manifest(
        repo.path(),
        &serde_json::json!({
            "fixes_applied": ["r14-anchor"],
            "entries": [{ "action": "created", "path": big }],
        }),
    );

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("byte cap"));
}

#[test]
fn audit_self_does_not_accept_a_not_good_gain_as_justification() {
    // held-out rose (+0.25) but the reward-hacking gap widened (+0.5): `aoa
    // eval compare` labels this pair not_good. The self-audit must consume
    // that same gate — a not_good pair cannot justify a context rise — and
    // surface the label in the JSON register.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let runs = TempDir::new().expect("tempdir");
    let baseline = runs.path().join("baseline.json");
    let migrated = runs.path().join("migrated.json");
    // baseline: visible 0.25, held-out 0.25 (gap 0).
    std::fs::write(
        &baseline,
        r#"{"tasks":[
            {"visible_success":true,"held_out_success":true},
            {"visible_success":false,"held_out_success":false},
            {"visible_success":false,"held_out_success":false},
            {"visible_success":false,"held_out_success":false}],
            "held_out_provenance":"native_composed","canaries":[]}"#,
    )
    .unwrap();
    // migrated: visible 1.0, held-out 0.5 (gap 0.5) — gap widened.
    std::fs::write(
        &migrated,
        r#"{"tasks":[
            {"visible_success":true,"held_out_success":true},
            {"visible_success":true,"held_out_success":true},
            {"visible_success":true,"held_out_success":false},
            {"visible_success":true,"held_out_success":false}],
            "held_out_provenance":"native_composed","canaries":[]}"#,
    )
    .unwrap();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(&baseline)
        .arg("--migrated")
        .arg(&migrated)
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a not_good pair must not clear the regression"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["held_out"]["label"], "not_good");
    assert_eq!(parsed["context_regression"], true);
}

#[test]
fn audit_self_surfaces_zero_canary_leak_shape_warning() {
    // The fixture pair is leak-shaped with zero canaries: `aoa eval compare`
    // warns zero_canary_leak_shape so the contamination-shaped gain never
    // passes silently. The self-audit consumes the same comparison and must
    // surface the warning in both registers even though the good label still
    // clears the regression.
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let output = aoa()
        .args(["audit", "--self", "--json", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["held_out"]["label"], "good");
    assert_eq!(
        parsed["held_out"]["warnings"][0], "zero_canary_leak_shape",
        "the leak-shaped warning must not be dropped: {parsed}"
    );

    aoa()
        .args(["audit", "--self", "--repo"])
        .arg(repo.path())
        .arg("--baseline")
        .arg(fixture("baseline.json"))
        .arg("--migrated")
        .arg(fixture("migrated.json"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains("ZeroCanaryLeakShape"));
}
