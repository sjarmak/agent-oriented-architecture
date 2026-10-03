use std::fs;

use aoa_migrate::{DeadImportFix, FixReport, MigrationPlan, NavigabilityAnchorFix};
use tempfile::TempDir;

fn only_report(plan: &MigrationPlan) -> &FixReport {
    assert_eq!(plan.reports.len(), 1, "one report per selected fix");
    &plan.reports[0]
}

#[test]
fn a_conforming_repo_reports_how_many_roots_were_examined() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("README.md"), "# repo\n").unwrap();
    fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

    let plan = MigrationPlan::build(dir.path(), &[&NavigabilityAnchorFix]).unwrap();

    assert!(plan.is_empty());
    assert!(plan.fix_ids.is_empty());
    let report = only_report(&plan);
    assert_eq!(report.fix_id, "navigability-anchor");
    assert!(report.ran);
    assert_eq!(report.examined, 1);
    assert_eq!(report.examined_unit, "package roots");
    assert_eq!(report.changes, 0);
    assert_eq!(
        report.no_change_reason.as_deref(),
        Some("none of the 1 package roots examined needed a change")
    );
}

#[test]
fn a_fix_that_plans_changes_reports_them_without_a_no_change_reason() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

    let plan = MigrationPlan::build(dir.path(), &[&NavigabilityAnchorFix]).unwrap();

    let report = only_report(&plan);
    assert!(report.ran);
    assert_eq!(report.changes, 1);
    assert_eq!(report.no_change_reason, None);
}

#[test]
fn an_ineligible_repo_reports_that_the_fix_did_not_run_and_why() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.ts"), "import { x } from './m';\n").unwrap();

    for (fix, reason) in [
        (DeadImportFix::rust(), "no Cargo.toml at the repo root"),
        (
            DeadImportFix::python(),
            "no pyproject.toml, setup.py or setup.cfg at the repo root",
        ),
        (
            DeadImportFix::typescript(),
            "no package.json or tsconfig.json at the repo root",
        ),
    ] {
        let plan = MigrationPlan::build(dir.path(), &[&fix]).unwrap();
        let report = only_report(&plan);
        assert!(!report.ran, "{} must report it did not run", report.fix_id);
        assert_eq!(report.examined, 0);
        assert_eq!(report.changes, 0);
        assert_eq!(report.no_change_reason.as_deref(), Some(reason));
    }
}

#[test]
fn an_eligible_repo_with_no_source_reports_nothing_to_examine() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("package.json"), "{}\n").unwrap();

    let plan = MigrationPlan::build(dir.path(), &[&DeadImportFix::typescript()]).unwrap();

    let report = only_report(&plan);
    assert!(report.ran);
    assert_eq!(report.examined, 0);
    assert_eq!(
        report.no_change_reason.as_deref(),
        Some("found no TypeScript/JavaScript source files to examine")
    );
}

#[test]
fn every_selected_fix_gets_a_report_in_selection_order() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("README.md"), "# repo\n").unwrap();

    let rust = DeadImportFix::rust();
    let plan = MigrationPlan::build(dir.path(), &[&NavigabilityAnchorFix, &rust]).unwrap();

    let ids: Vec<&str> = plan.reports.iter().map(|r| r.fix_id.as_str()).collect();
    assert_eq!(ids, ["navigability-anchor", "dead-imports"]);
}
