//! End-to-end acceptance: plan is read-only, apply lands the anchor and clears
//! the audit finding, and rollback returns the checkout to baseline.

use std::fs;

use aoa_migrate::{apply, rollback, CodeFix, MigrationPlan, NavigabilityAnchorFix};
use tempfile::TempDir;

/// A fixture repo: a manifest-bearing root with source files but no README, so
/// the audit reports it as a navigability site.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let p = dir.path();
    fs::write(p.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(p.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    dir
}

fn nav_count(repo: &std::path::Path) -> u64 {
    let cfg = aoa_audit::AuditConfig::default();
    let report = aoa_audit::audit(repo, &cfg).unwrap();
    report
        .items
        .iter()
        .filter(|i| i.title.contains("navigability anchor"))
        .map(|i| i.measured_cost.value)
        .sum()
}

#[test]
fn plan_previews_without_writing() {
    let dir = fixture();
    let repo = dir.path();

    let plan = MigrationPlan::build(repo, &[&NavigabilityAnchorFix]).unwrap();
    assert!(!plan.is_empty(), "the README-less root should be planned");
    let diff = plan.render_diff();
    assert!(diff.contains("create"));
    assert!(diff.contains("README.md"));
    // Nothing was written by planning.
    assert!(!repo.join("README.md").exists());
}

#[test]
fn apply_clears_the_audit_finding_then_rollback_restores_baseline() {
    let dir = fixture();
    let repo = dir.path();

    assert!(
        nav_count(repo) >= 1,
        "fixture starts with a navigability gap"
    );

    let plan = MigrationPlan::build(repo, &[&NavigabilityAnchorFix]).unwrap();
    apply(repo, &plan).unwrap();

    assert!(repo.join("README.md").exists(), "anchor created");
    // The audit independently re-verifies the spec is met (verify, not define).
    assert_eq!(
        nav_count(repo),
        0,
        "navigability finding cleared after apply"
    );

    rollback(repo).unwrap();
    assert!(
        !repo.join("README.md").exists(),
        "rollback removed the anchor"
    );
    assert!(nav_count(repo) >= 1, "finding returns after rollback");
}

#[test]
fn applied_anchor_is_idempotent_and_reproducible() {
    // Two fixtures with identical structure but different file bodies must yield
    // byte-identical anchors (oracle-blind + reproducible).
    let a = fixture();
    let b = fixture();
    fs::write(
        a.path().join("src/lib.rs"),
        "pub fn secret() -> u32 { 1234 }\n",
    )
    .unwrap();
    fs::write(b.path().join("src/lib.rs"), "// nothing here\n").unwrap();

    let plan_a = NavigabilityAnchorFix.plan(a.path()).unwrap();
    let plan_b = NavigabilityAnchorFix.plan(b.path()).unwrap();
    assert_eq!(plan_a.len(), plan_b.len());
    // Compare the generated bodies (skip the title line, which is the temp dir
    // name and legitimately differs between the two fixtures).
    let body = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert_eq!(
        body(&plan_a[0].new_content),
        body(&plan_b[0].new_content),
        "anchor body must not depend on file contents"
    );
}
