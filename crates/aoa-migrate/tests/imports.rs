//! End-to-end acceptance for [`DeadImportFix`] against a *real* `cargo check`.
//!
//! These tests invoke the toolchain on a throwaway, dependency-free crate so
//! `--offline` is trivially satisfied (no registry needed). They run inside
//! `cargo test`, so `cargo` is always on `PATH`; if a constrained sandbox ever
//! makes the toolchain unavailable, the fix surfaces a loud
//! `ToolchainUnavailable`/`BuildFailed` and the assertions below would catch a
//! silent empty plan rather than passing vacuously.

use std::fs;

use aoa_migrate::{apply, rollback, ChangeAction, DeadImportFix, MigrationPlan};
use tempfile::TempDir;

mod common;
use common::tree_snapshot;

/// A minimal, dependency-free cargo crate carrying one plainly-unused import.
fn crate_with_unused_import() -> TempDir {
    let dir = TempDir::new().unwrap();
    let p = dir.path();
    fs::write(p.join("Cargo.toml"), CARGO_TOML).unwrap();
    fs::create_dir_all(p.join("src")).unwrap();
    fs::write(
        p.join("src/lib.rs"),
        "use std::collections::HashMap;\n\npub fn answer() -> u32 {\n    42\n}\n",
    )
    .unwrap();
    dir
}

const CARGO_TOML: &str = "[package]\nname = \"deadimport-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n";

#[test]
fn plan_removes_unused_import_and_leaves_real_repo_byte_unchanged() {
    let dir = crate_with_unused_import();
    let repo = dir.path();
    let lib = repo.join("src/lib.rs");
    let before = fs::read_to_string(&lib).unwrap();
    let before_entries = tree_snapshot(repo);

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::rust()]).unwrap();

    // Exactly one Overwrite change, dropping the unused import.
    assert_eq!(plan.changes.len(), 1, "one file changed");
    let change = &plan.changes[0];
    assert_eq!(change.action, ChangeAction::Overwrite);
    assert_eq!(change.path, lib);
    assert!(
        !change.new_content.contains("HashMap"),
        "the unused import is gone from the planned content"
    );
    assert!(
        change.new_content.contains("pub fn answer"),
        "real code is preserved"
    );
    assert_eq!(
        change.old_content.as_deref(),
        Some(before.as_str()),
        "old_content is the real repo's current bytes"
    );

    // C3 read-only parity: planning wrote nothing to the real repo and left no
    // temp residue under it.
    assert_eq!(
        fs::read_to_string(&lib).unwrap(),
        before,
        "real file untouched"
    );
    assert_eq!(
        tree_snapshot(repo),
        before_entries,
        "no files added/removed in the repo"
    );
    assert!(
        !repo.join(".aoa").exists(),
        "no migration bookkeeping from a dry-run plan"
    );

    // Provenance recorded (verification half of reproducibility).
    assert_eq!(plan.provenance.len(), 1);
    assert_eq!(plan.provenance[0].fix_id, "dead-imports");
    assert!(
        plan.provenance[0].toolchain.contains("rustc"),
        "toolchain identity captured: {}",
        plan.provenance[0].toolchain
    );
}

#[test]
fn plan_is_byte_identical_across_runs() {
    // AC4: deterministic PlannedChanges on a fixed toolchain. (Cross-toolchain
    // reproducibility is the pin's job and is recorded as provenance, not
    // asserted here — a single-machine test cannot vary the toolchain.)
    let dir = crate_with_unused_import();
    let repo = dir.path();

    let a = MigrationPlan::build(repo, &[&DeadImportFix::rust()]).unwrap();
    let b = MigrationPlan::build(repo, &[&DeadImportFix::rust()]).unwrap();
    assert_eq!(a.changes, b.changes, "planned changes are byte-identical");
}

#[test]
fn apply_writes_overwrite_rebuilds_clean_then_rollback_restores_baseline() {
    let dir = crate_with_unused_import();
    let repo = dir.path();
    let lib = repo.join("src/lib.rs");
    let baseline = fs::read_to_string(&lib).unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::rust()]).unwrap();
    apply(repo, &plan).unwrap();

    // The Overwrite landed and the import is gone from disk.
    let applied = fs::read_to_string(&lib).unwrap();
    assert!(
        !applied.contains("HashMap"),
        "applied file has no unused import"
    );
    assert_ne!(applied, baseline);

    // Re-planning the now-clean crate finds nothing further to remove (the audit
    // signal the fix verifies against: a clean build => empty plan).
    let replan = MigrationPlan::build(repo, &[&DeadImportFix::rust()]).unwrap();
    assert!(replan.is_empty(), "no unused imports remain after apply");

    rollback(repo).unwrap();
    assert_eq!(
        fs::read_to_string(&lib).unwrap(),
        baseline,
        "rollback restores the original import from the archive"
    );
}

#[test]
fn feature_gated_import_is_preserved_under_all_features() {
    // H-B: a `use` referenced only under `#[cfg(feature = "x")]` would look
    // unused with the feature off; `--all-features` activates it so the import
    // is NOT removed. Proves the fix does not blindly strip cfg-gated imports
    // (for the activatable-feature case).
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"feat-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[features]\nextra = []\n\n[dependencies]\n",
    )
    .unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(
        repo.join("src/lib.rs"),
        "#[cfg(feature = \"extra\")]\nuse std::collections::HashMap;\n\n#[cfg(feature = \"extra\")]\npub fn build() -> HashMap<u32, u32> {\n    HashMap::new()\n}\n",
    )
    .unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::rust()]).unwrap();
    assert!(
        plan.is_empty(),
        "the feature-gated import is used under --all-features and must be kept; got {:?}",
        plan.changes
    );
}
