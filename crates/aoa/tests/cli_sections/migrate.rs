use super::*;

// --- aoa migrate (aoa-mnz.2) ------------------------------------------------

/// A fixture checkout with a manifest-bearing root but no README, so the audit
/// reports a navigability site the migration can fix.
pub(super) fn migrate_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let p = dir.path();
    std::fs::write(p.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(p.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    dir
}

#[test]
fn migrate_plan_is_dry_run_and_writes_nothing() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("dry-run"))
        .stdout(predicate::str::contains("README.md"));
    assert!(
        !repo.path().join("README.md").exists(),
        "dry-run must not write the anchor"
    );
}

#[test]
fn migrate_apply_then_rollback_round_trips() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        // The human verify line attributes the re-audit to the navigability
        // fix explicitly, so it cannot be read as covering the dead-import
        // fixes the re-audit does not measure.
        .stdout(predicate::str::contains(
            "Re-audit (navigability-anchor) verifies 0 navigability site(s) remaining",
        ));
    assert!(
        repo.path().join("README.md").exists(),
        "apply writes the anchor"
    );

    aoa()
        .args(["migrate", "--rollback", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
    assert!(
        !repo.path().join("README.md").exists(),
        "rollback restores the baseline"
    );
}

#[test]
fn migrate_apply_json_reports_verified_remaining_zero() {
    let repo = migrate_repo();
    let assert = aoa()
        .args(["migrate", "--apply", "--json", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["mode"], "apply");
    // Present (not null) because the navigability fix ran and was re-audited;
    // the count it re-measured is zero.
    assert_eq!(v["navigability_sites_remaining"], 0);
    // Per-fix eligibility: the navigability fix's note is tagged with its id.
    let notes = v["eligibility_notes"]
        .as_array()
        .expect("eligibility_notes");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["fix_id"], "navigability-anchor");
    assert!(notes[0]["note"].as_str().unwrap().contains("code-layer"));
}

#[test]
fn migrate_apply_json_navigability_remaining_is_null_when_nav_fix_excluded() {
    // When the navigability fix is excluded via --fix, its re-audit count is
    // not applicable. The JSON field must serialize as null (not 0, not
    // absent) so a consumer can distinguish "not measured" from "measured
    // zero" — the contract the Option<u64> change introduced.
    let repo = migrate_repo();
    let assert = aoa()
        .args([
            "migrate",
            "--apply",
            "--json",
            "--fix",
            "dead-imports",
            "--repo",
        ])
        .arg(repo.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["mode"], "apply");
    assert!(
        v["navigability_sites_remaining"].is_null(),
        "expected null when the navigability fix did not run, got {:?}",
        v["navigability_sites_remaining"]
    );
    // The navigability anchor must not have been written (fix was excluded).
    assert!(
        !repo.path().join("README.md").exists(),
        "navigability fix was excluded via --fix, so no anchor should be written"
    );
}

#[test]
fn migrate_fix_selector_rejects_unknown_id() {
    let repo = migrate_repo();
    aoa()
        .args(["migrate", "--fix", "no-such-fix", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown fix id"));
}

#[test]
fn migrate_fix_selector_runs_named_fix() {
    let repo = migrate_repo();
    aoa()
        .args([
            "migrate",
            "--fix",
            "navigability-anchor",
            "--apply",
            "--repo",
        ])
        .arg(repo.path())
        .assert()
        .success();
    assert!(
        repo.path().join("README.md").exists(),
        "selected fix ran and wrote the anchor"
    );
}

fn conforming_repo_without_language_markers() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("README.md"), "# demo\n").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "plain\n").unwrap();
    dir
}

#[test]
fn migrate_plan_json_reports_every_fix_on_a_zero_change_result() {
    let repo = conforming_repo_without_language_markers();
    let assert = aoa()
        .args(["migrate", "--json", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["mode"], "plan");
    assert_eq!(v["changes"].as_array().expect("changes").len(), 0);

    let reports = v["fix_reports"].as_array().expect("fix_reports");
    let ids: Vec<&str> = reports
        .iter()
        .map(|r| r["fix_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "navigability-anchor",
            "dead-imports",
            "dead-imports-python",
            "dead-imports-typescript"
        ]
    );

    assert_eq!(reports[0]["ran"], true);
    assert_eq!(reports[0]["examined"], 1);
    assert_eq!(reports[0]["examined_unit"], "package roots");
    assert_eq!(reports[0]["changes"], 0);
    assert_eq!(
        reports[0]["no_change_reason"],
        "none of the 1 package roots examined needed a change"
    );

    assert_eq!(reports[3]["ran"], false);
    assert_eq!(reports[3]["examined"], 0);
    assert_eq!(
        reports[3]["no_change_reason"],
        "no package.json or tsconfig.json at the repo root"
    );
}

#[test]
fn migrate_plan_human_reports_every_fix_on_a_zero_change_result() {
    let repo = conforming_repo_without_language_markers();
    aoa()
        .args(["migrate", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No changes planned."))
        .stdout(predicate::str::contains(
            "[fix:navigability-anchor] ran: examined 1 package roots, planned 0 change(s): \
             none of the 1 package roots examined needed a change",
        ))
        .stdout(predicate::str::contains(
            "[fix:dead-imports-typescript] did not run: \
             no package.json or tsconfig.json at the repo root",
        ));
}

#[test]
fn migrate_apply_reports_every_fix_when_nothing_was_planned() {
    let repo = conforming_repo_without_language_markers();
    aoa()
        .args(["migrate", "--apply", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to apply"))
        .stdout(predicate::str::contains(
            "[fix:dead-imports] did not run: no Cargo.toml at the repo root",
        ));

    let assert = aoa()
        .args(["migrate", "--apply", "--json", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    let v: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(v["fix_reports"].as_array().expect("fix_reports").len(), 4);
    assert!(!repo.path().join(".aoa").exists());
}
