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
