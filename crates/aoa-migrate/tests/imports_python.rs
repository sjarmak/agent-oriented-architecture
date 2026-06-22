//! End-to-end acceptance for the Python [`DeadImportFix`] adapter against a real
//! `ruff`.
//!
//! Unlike `cargo` (always on `PATH` under `cargo test`), `ruff` may be absent on a
//! developer machine, so each test probes for it and skips with a printed notice
//! rather than failing. A skip is NOT a vacuous pass: the assertions below would
//! still catch a silent empty plan if `ruff` were present but the adapter were
//! broken. CI must install a pinned `ruff` so these execute rather than skip.

use std::fs;
use std::process::Command;

use aoa_migrate::{apply, rollback, ChangeAction, DeadImportFix, MigrateError, MigrationPlan};
use tempfile::TempDir;

mod common;
use common::tree_snapshot;

/// `true` when `ruff` is invocable. Tests short-circuit (with a notice) otherwise.
fn ruff_available() -> bool {
    Command::new("ruff")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

macro_rules! require_ruff {
    ($name:literal) => {
        if !ruff_available() {
            eprintln!("SKIP {}: `ruff` not on PATH", $name);
            return;
        }
    };
}

const PYPROJECT: &str = "[project]\nname = \"deadimport-fixture\"\nversion = \"0.0.0\"\n";

/// A minimal Python package carrying one plainly-unused import (`os`) and a piece
/// of real code, written at `src_rel`.
fn py_pkg_with_unused_import(src_rel: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("pyproject.toml"), PYPROJECT).unwrap();
    fs::write(
        dir.path().join(src_rel),
        "import os\nimport sys\n\n\ndef answer() -> int:\n    return len(sys.path)\n",
    )
    .unwrap();
    dir
}

#[test]
fn plan_removes_unused_import_and_leaves_real_repo_byte_unchanged() {
    require_ruff!("plan_removes_unused_import");
    let dir = py_pkg_with_unused_import("app.py");
    let repo = dir.path();
    let app = repo.join("app.py");
    let before = fs::read_to_string(&app).unwrap();
    let before_entries = tree_snapshot(repo);

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();

    assert_eq!(plan.changes.len(), 1, "one file changed");
    let change = &plan.changes[0];
    assert_eq!(change.action, ChangeAction::Overwrite);
    assert_eq!(change.path, app);
    assert!(
        !change.new_content.contains("import os"),
        "the unused `import os` is gone from the planned content"
    );
    assert!(
        change.new_content.contains("import sys"),
        "the used `import sys` is preserved"
    );
    assert!(
        change.new_content.contains("def answer"),
        "real code is preserved"
    );
    assert_eq!(
        change.old_content.as_deref(),
        Some(before.as_str()),
        "old_content is the real repo's current bytes"
    );

    // Read-only parity: planning wrote nothing to the real repo, no temp residue.
    assert_eq!(
        fs::read_to_string(&app).unwrap(),
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

    // Provenance recorded.
    assert_eq!(plan.provenance.len(), 1);
    assert_eq!(plan.provenance[0].fix_id, "dead-imports-python");
    assert!(
        plan.provenance[0].toolchain.contains("ruff"),
        "ruff identity captured: {}",
        plan.provenance[0].toolchain
    );
}

#[test]
fn plan_is_byte_identical_across_runs() {
    require_ruff!("plan_is_byte_identical");
    let dir = py_pkg_with_unused_import("app.py");
    let repo = dir.path();
    let a = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    let b = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    assert_eq!(a.changes, b.changes, "planned changes are byte-identical");
}

#[test]
fn apply_then_replan_finds_nothing_then_rollback_restores() {
    require_ruff!("apply_then_replan_rollback");
    let dir = py_pkg_with_unused_import("app.py");
    let repo = dir.path();
    let app = repo.join("app.py");
    let baseline = fs::read_to_string(&app).unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    apply(repo, &plan).unwrap();

    let applied = fs::read_to_string(&app).unwrap();
    assert!(
        !applied.contains("import os"),
        "applied file has no unused import"
    );
    assert_ne!(applied, baseline);

    let replan = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    assert!(replan.is_empty(), "no unused imports remain after apply");

    rollback(repo).unwrap();
    assert_eq!(
        fs::read_to_string(&app).unwrap(),
        baseline,
        "rollback restores the original import from the archive"
    );
}

#[test]
fn type_checking_used_import_is_preserved() {
    // The Python analogue of the Rust cfg-gated-import test: an `if TYPE_CHECKING:`
    // import referenced in an annotation is genuinely used, so ruff keeps it and
    // the plan is empty. (An *unreferenced* TYPE_CHECKING import IS removed — that
    // is the declared UNCHECKED exclusion, not asserted here.)
    require_ruff!("type_checking_preserved");
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(repo.join("pyproject.toml"), PYPROJECT).unwrap();
    fs::write(
        repo.join("app.py"),
        "from __future__ import annotations\nfrom typing import TYPE_CHECKING\n\nif TYPE_CHECKING:\n    from collections import OrderedDict\n\n\ndef f(x: OrderedDict) -> None:\n    pass\n",
    )
    .unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    assert!(
        plan.is_empty(),
        "a referenced TYPE_CHECKING import must be kept; got {:?}",
        plan.changes
    );
}

#[test]
fn unused_variable_is_not_removed() {
    // Scope guard (discipline #2): only F401 is selected, so an unused local
    // (F841) is left untouched while the unused import is removed. The analogue of
    // the Rust edition-rewrite rejection — proof this is not a blind autofixer.
    require_ruff!("unused_variable_not_removed");
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(repo.join("pyproject.toml"), PYPROJECT).unwrap();
    fs::write(
        repo.join("app.py"),
        "import os\n\n\ndef f():\n    unused = 1\n",
    )
    .unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    assert_eq!(plan.changes.len(), 1);
    let new = &plan.changes[0].new_content;
    assert!(!new.contains("import os"), "unused import removed");
    assert!(
        new.contains("unused = 1"),
        "unused local NOT removed (out of scope)"
    );
}

#[test]
fn isolated_blocks_a_config_that_would_disable_the_fix() {
    // Construct-validity guard: a repo `[tool.ruff.lint]` setting `unfixable =
    // ["F401"]` would, if honored, prevent removal. `--isolated` ignores it, so the
    // fix still applies — proving the repo cannot widen OR narrow our scope.
    require_ruff!("isolated_blocks_config");
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(
        repo.join("pyproject.toml"),
        "[project]\nname = \"x\"\nversion = \"0\"\n[tool.ruff.lint]\nunfixable = [\"F401\"]\n",
    )
    .unwrap();
    fs::write(repo.join("app.py"), "import os\nx = 1\n").unwrap();

    let plan = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap();
    assert_eq!(
        plan.changes.len(),
        1,
        "the malicious config did not block the fix"
    );
    assert!(!plan.changes[0].new_content.contains("import os"));
}

#[test]
fn syntactically_broken_python_is_a_loud_repo_does_not_check() {
    require_ruff!("broken_python_loud");
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    fs::write(repo.join("pyproject.toml"), PYPROJECT).unwrap();
    fs::write(repo.join("app.py"), "def f(:\n").unwrap();

    let err = MigrationPlan::build(repo, &[&DeadImportFix::python()]).unwrap_err();
    assert!(
        matches!(err, MigrateError::RepoDoesNotCheck { .. }),
        "a tree that does not parse is a LOUD error, not an empty plan; got {err:?}"
    );
}
