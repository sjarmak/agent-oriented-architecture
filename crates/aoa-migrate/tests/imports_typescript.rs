//! End-to-end acceptance for the TypeScript/JS [`DeadImportFix`] adapter against
//! the vendored, pinned ESLint toolchain.
//!
//! These tests need `node` on `PATH` and the vendored ESLint installed. The
//! `node_modules` tree is gitignored and regenerated with `npm ci` in
//! `assets/eslint/`, so a fresh clone has `node` but no ESLint until that runs.
//! Either prerequisite missing skips with a printed notice rather than failing,
//! but a skip is not a vacuous pass: the assertions would still catch a silent
//! empty plan if the adapter were broken. CI must install `node` and run
//! `npm ci` so these execute.

use std::fs;
use std::path::Path;
use std::process::Command;

use aoa_migrate::{apply, rollback, ChangeAction, DeadImportFix, MigrateError, MigrationPlan};
use tempfile::TempDir;

mod common;
use common::tree_snapshot;

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn vendored_eslint_available() -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/eslint/node_modules/eslint/bin/eslint.js")
        .is_file()
}

macro_rules! require_node {
    ($name:literal) => {
        if !node_available() {
            eprintln!("SKIP {}: `node` not on PATH", $name);
            return;
        }
        if !vendored_eslint_available() {
            eprintln!(
                "SKIP {}: vendored ESLint absent; run `npm ci` in crates/aoa-migrate/assets/eslint",
                $name
            );
            return;
        }
    };
}

/// A minimal TS project: `package.json` marker plus one module importing both a
/// used and an unused symbol, with an unused local to prove scope.
fn ts_project() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("package.json"),
        "{\n  \"name\": \"fixture\"\n}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("a.ts"),
        "import { used, unused } from './m';\n\nexport function go(): number {\n  const dead = 1;\n  return used();\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn plan_removes_unused_import_and_leaves_real_repo_byte_unchanged() {
    require_node!("plan_removes_unused_import");
    let dir = ts_project();
    let repo = dir.path();
    let a = repo.join("a.ts");
    let before = fs::read_to_string(&a).unwrap();
    let before_entries = tree_snapshot(repo);

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();

    assert_eq!(plan.changes.len(), 1, "one file changed");
    let change = &plan.changes[0];
    assert_eq!(change.action, ChangeAction::Overwrite);
    assert_eq!(change.path, a);
    assert!(
        !change.new_content.contains("unused"),
        "the unused import specifier is gone: {:?}",
        change.new_content
    );
    assert!(
        change.new_content.contains("used"),
        "the used import is preserved"
    );
    assert_eq!(change.old_content.as_deref(), Some(before.as_str()));

    // Read-only parity.
    assert_eq!(
        fs::read_to_string(&a).unwrap(),
        before,
        "real file untouched"
    );
    assert_eq!(
        tree_snapshot(repo),
        before_entries,
        "repo structurally unchanged"
    );
    assert!(
        !repo.join(".aoa").exists(),
        "no bookkeeping from a dry-run plan"
    );

    // Provenance pins node + eslint + plugin + config fingerprint.
    assert_eq!(plan.provenance.len(), 1);
    assert_eq!(plan.provenance[0].fix_id, "dead-imports-typescript");
    let tc = &plan.provenance[0].toolchain;
    assert!(
        tc.contains("node") && tc.contains("eslint") && tc.contains("config-fp"),
        "toolchain identity captured: {tc}"
    );
}

#[test]
fn plan_is_byte_identical_across_runs() {
    require_node!("plan_is_byte_identical");
    let dir = ts_project();
    let repo = dir.path();
    let a = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    let b = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    assert_eq!(a.changes, b.changes, "planned changes are byte-identical");
}

#[test]
fn apply_then_replan_finds_nothing_then_rollback_restores() {
    require_node!("apply_then_replan_rollback");
    let dir = ts_project();
    let repo = dir.path();
    let a = repo.join("a.ts");
    let baseline = fs::read_to_string(&a).unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    apply(repo, &plan).unwrap();

    let applied = fs::read_to_string(&a).unwrap();
    assert!(
        !applied.contains("unused"),
        "applied file has no unused import"
    );
    assert_ne!(applied, baseline);

    let replan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    assert!(replan.is_empty(), "no unused imports remain after apply");

    rollback(repo).unwrap();
    assert_eq!(
        fs::read_to_string(&a).unwrap(),
        baseline,
        "rollback restores baseline"
    );
}

#[test]
fn unused_local_is_not_removed() {
    // Scope guard: only `unused-imports/no-unused-imports` is enabled, so the
    // unused `const dead` is left untouched while the unused import is removed.
    require_node!("unused_local_not_removed");
    let dir = ts_project();
    let repo = dir.path();
    let plan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    assert_eq!(plan.changes.len(), 1);
    let new = &plan.changes[0].new_content;
    assert!(!new.contains("unused"), "unused import removed");
    assert!(
        new.contains("const dead = 1"),
        "unused local NOT removed (out of scope)"
    );
}

#[test]
fn dash_prefixed_filename_is_not_treated_as_a_flag() {
    // Argument-injection guard: a source file whose name starts with `--` must
    // reach ESLint as a path (after the `--` terminator), not be parsed as an
    // option. Without the terminator, `--inject.ts` aborts the run (exit 2).
    require_node!("dash_prefixed_filename");
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(repo.join("package.json"), "{\n  \"name\": \"fixture\"\n}\n").unwrap();
    fs::write(
        repo.join("--inject.ts"),
        "import { unused } from './m';\nexport const z = 1;\n",
    )
    .unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    assert_eq!(
        plan.changes.len(),
        1,
        "the --prefixed file was linted, not rejected"
    );
    assert_eq!(plan.changes[0].path, repo.join("--inject.ts"));
    assert!(!plan.changes[0].new_content.contains("unused"));
}

#[test]
fn repo_eslint_config_cannot_disable_the_fix() {
    // Hermeticity guard: a repo `eslint.config.mjs` that turns the rule off would,
    // if honored, prevent removal. `--no-config-lookup` + our `--config` ignore it,
    // so the fix still applies — the repo cannot widen OR narrow our scope.
    require_node!("repo_config_ignored");
    let dir = ts_project();
    let repo = dir.path();
    fs::write(
        repo.join("eslint.config.mjs"),
        "export default [{ rules: { 'unused-imports/no-unused-imports': 'off' } }];\n",
    )
    .unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    assert_eq!(
        plan.changes.len(),
        1,
        "the repo config did not block the fix"
    );
    assert!(!plan.changes[0].new_content.contains("unused"));
}

#[test]
fn parsing_error_is_a_loud_repo_does_not_check() {
    require_node!("parse_error_loud");
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(repo.join("package.json"), "{}\n").unwrap();
    fs::write(repo.join("a.ts"), "const x = ;\n").unwrap();

    let err = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap_err();
    assert!(
        matches!(err, MigrateError::RepoDoesNotCheck { .. }),
        "a tree that does not parse is a LOUD error, not an empty plan; got {err:?}"
    );
}

#[test]
fn eligible_project_with_no_source_is_an_empty_plan() {
    // No `node` needed: the adapter returns empty before invoking ESLint when an
    // eligible project carries no source files.
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(repo.join("package.json"), "{}\n").unwrap();
    let plan = MigrationPlan::build(repo, &[&DeadImportFix::typescript()]).unwrap();
    assert!(plan.is_empty(), "no source files => legitimate empty plan");
}
