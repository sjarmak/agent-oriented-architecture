//! Applying a [`MigrationPlan`] reversibly, and rolling it back.
//!
//! Apply is **plan-first**: the manifest listing every target is persisted
//! *before* any file is written, so an interrupted apply still leaves a record
//! that [`rollback`] can use to return the checkout to its baseline. Created
//! files are deleted on rollback; overwritten files are archived before being
//! replaced and restored from that archive on rollback.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::MigrateError;
use crate::fix::{ChangeAction, FixEligibility};
use crate::plan::MigrationPlan;

/// Directory (under the repo, inside the ignored `.aoa/` tree) holding the
/// manifest and archived originals. Living in `.aoa/` keeps the migration's
/// bookkeeping out of the tracked tree the treatment is measured on, and the
/// audit's hidden-dir skip means re-auditing never mistakes an archive for
/// source.
const MIGRATE_DIR: &str = ".aoa/migrate";
const MANIFEST_NAME: &str = "manifest.json";
const ARCHIVE_DIR: &str = "archive";

/// One recorded change in a [`MigrationManifest`]. The variant determines how
/// rollback undoes it: a [`Created`](ManifestEntry::Created) file is deleted, a
/// [`Modified`](ManifestEntry::Modified) file is restored from its archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ManifestEntry {
    /// A file the migration created (no prior version existed).
    Created { path: PathBuf },
    /// A file the migration replaced; `archive` holds the original.
    Modified { path: PathBuf, archive: PathBuf },
}

/// The durable record of an applied migration: which fixes ran, every file
/// touched, and each contributing fix's eligibility precondition. Serialized to
/// `.aoa/migrate/manifest.json` and consumed by [`rollback`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationManifest {
    /// Ids of the fixes that contributed changes (provenance for pre-registration).
    pub fixes_applied: Vec<String>,
    /// Every file the migration writes, in apply order.
    pub entries: Vec<ManifestEntry>,
    /// Each contributing fix's R0 eligibility precondition, tagged with its id.
    /// `#[serde(default)]` keeps a manifest written before per-fix notes existed
    /// (a bare older schema) readable, so rollback never breaks on an in-flight
    /// migration from an earlier binary.
    #[serde(default)]
    pub eligibility_notes: Vec<FixEligibility>,
}

/// Apply `plan` to `repo`, returning the manifest that records it.
///
/// Writes are create-exclusive: if a [`Create`](ChangeAction::Create) target is
/// found already present, the apply fails with [`MigrateError::TargetExists`]
/// rather than clobbering repo content. The manifest is persisted before any
/// file is written so rollback works even after an interrupted apply.
pub fn apply(repo: &Path, plan: &MigrationPlan) -> Result<MigrationManifest, MigrateError> {
    let migrate_dir = repo.join(MIGRATE_DIR);
    let archive_root = migrate_dir.join(ARCHIVE_DIR);

    // Refuse to clobber a prior migration's rollback record: an existing
    // manifest means a migration is applied but not rolled back, and writing a
    // fresh manifest would orphan its archived originals.
    if manifest_path(repo).exists() {
        return Err(MigrateError::AlreadyApplied {
            manifest: manifest_path(repo),
        });
    }

    // 1. Derive the manifest entries, validating create-exclusivity and that
    // every target stays inside the repo up front.
    let mut entries = Vec::with_capacity(plan.changes.len());
    for change in &plan.changes {
        let rel = change
            .path
            .strip_prefix(repo)
            .map_err(|_| MigrateError::PathOutsideRepo {
                path: change.path.clone(),
                repo: repo.to_path_buf(),
            })?;
        match change.action {
            ChangeAction::Create => {
                if change.path.exists() {
                    return Err(MigrateError::TargetExists {
                        path: change.path.clone(),
                    });
                }
                entries.push(ManifestEntry::Created {
                    path: change.path.clone(),
                });
            }
            ChangeAction::Overwrite => {
                entries.push(ManifestEntry::Modified {
                    path: change.path.clone(),
                    archive: archive_root.join(rel),
                });
            }
        }
    }

    let manifest = MigrationManifest {
        fixes_applied: plan.fix_ids.clone(),
        entries,
        eligibility_notes: plan.eligibility_notes.clone(),
    };

    // 2. Plan-first: persist the manifest before touching any source file.
    create_dir_all(&migrate_dir)?;
    write_manifest(repo, &manifest)?;

    // 3. Execute. entries[i] corresponds to plan.changes[i] (same order).
    for (entry, change) in manifest.entries.iter().zip(&plan.changes) {
        match entry {
            ManifestEntry::Created { path } => {
                write_create_new(path, &change.new_content)?;
            }
            ManifestEntry::Modified { path, archive } => {
                // Archive the original *before* replacing it, so a crash between
                // the two steps still leaves the original recoverable.
                if let Some(parent) = archive.parent() {
                    create_dir_all(parent)?;
                }
                copy(path, archive)?;
                write_replace(path, &change.new_content)?;
            }
        }
    }

    Ok(manifest)
}

/// Roll back the migration recorded in `repo`'s manifest, restoring the
/// baseline checkout. Created files are deleted; modified files are restored
/// from their archive. Undoes changes in reverse apply order, then removes the
/// migration bookkeeping. A missing archive for a modified entry means its write
/// never landed (archive precedes write), so the original is already intact.
pub fn rollback(repo: &Path) -> Result<(), MigrateError> {
    let manifest = read_manifest(repo)?;
    for entry in manifest.entries.iter().rev() {
        match entry {
            ManifestEntry::Created { path } => remove_file_if_present(path)?,
            ManifestEntry::Modified { path, archive } => {
                if archive.exists() {
                    copy(archive, path)?;
                }
            }
        }
    }
    remove_dir_all_if_present(&repo.join(MIGRATE_DIR))?;
    Ok(())
}

/// Path of the migration manifest under `repo`.
pub fn manifest_path(repo: &Path) -> PathBuf {
    repo.join(MIGRATE_DIR).join(MANIFEST_NAME)
}

/// Read and parse the migration manifest from `repo`.
pub fn read_manifest(repo: &Path) -> Result<MigrationManifest, MigrateError> {
    let path = manifest_path(repo);
    let raw = std::fs::read_to_string(&path).map_err(|source| io_err(&path, source))?;
    serde_json::from_str(&raw).map_err(|source| MigrateError::Manifest { path, source })
}

fn write_manifest(repo: &Path, manifest: &MigrationManifest) -> Result<(), MigrateError> {
    let path = manifest_path(repo);
    let raw = serde_json::to_string_pretty(manifest).map_err(|source| MigrateError::Manifest {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, raw).map_err(|source| io_err(&path, source))
}

fn write_create_new(path: &Path, content: &str) -> Result<(), MigrateError> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                MigrateError::TargetExists {
                    path: path.to_path_buf(),
                }
            } else {
                io_err(path, source)
            }
        })?;
    file.write_all(content.as_bytes())
        .map_err(|source| io_err(path, source))
}

fn write_replace(path: &Path, content: &str) -> Result<(), MigrateError> {
    std::fs::write(path, content).map_err(|source| io_err(path, source))
}

fn copy(from: &Path, to: &Path) -> Result<(), MigrateError> {
    std::fs::copy(from, to).map_err(|source| MigrateError::Copy {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn create_dir_all(dir: &Path) -> Result<(), MigrateError> {
    std::fs::create_dir_all(dir).map_err(|source| io_err(dir, source))
}

fn remove_file_if_present(path: &Path) -> Result<(), MigrateError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_err(path, source)),
    }
}

fn remove_dir_all_if_present(dir: &Path) -> Result<(), MigrateError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_err(dir, source)),
    }
}

fn io_err(path: &Path, source: std::io::Error) -> MigrateError {
    MigrateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fix::PlannedChange;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aoa-migrate-apply-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_plan(path: PathBuf, content: &str) -> MigrationPlan {
        MigrationPlan {
            changes: vec![PlannedChange {
                path,
                action: ChangeAction::Create,
                new_content: content.to_string(),
                old_content: None,
            }],
            fix_ids: vec!["test-fix".to_string()],
            eligibility_notes: vec![FixEligibility {
                fix_id: "test-fix".to_string(),
                note: "test eligibility".to_string(),
            }],
        }
    }

    #[test]
    fn apply_creates_file_and_records_manifest() {
        let repo = tmp("apply-create");
        let target = repo.join("README.md");
        let manifest = apply(&repo, &create_plan(target.clone(), "# hi\n")).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "# hi\n");
        assert_eq!(manifest.fixes_applied, vec!["test-fix"]);
        assert!(matches!(manifest.entries[0], ManifestEntry::Created { .. }));
        assert!(manifest_path(&repo).exists(), "manifest persisted");
        assert_eq!(manifest.eligibility_notes[0].fix_id, "test-fix");
        assert_eq!(manifest.eligibility_notes[0].note, "test eligibility");
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn apply_refuses_when_a_manifest_already_exists() {
        let repo = tmp("apply-already");
        apply(&repo, &create_plan(repo.join("A.md"), "# a\n")).unwrap();

        // A second apply (different target) must not clobber the first
        // migration's rollback record.
        let err = apply(&repo, &create_plan(repo.join("B.md"), "# b\n")).unwrap_err();
        assert!(matches!(err, MigrateError::AlreadyApplied { .. }));
        assert!(!repo.join("B.md").exists(), "second apply wrote nothing");
        // The first migration's manifest still describes A, so rollback works.
        assert!(matches!(
            read_manifest(&repo).unwrap().entries[0],
            ManifestEntry::Created { .. }
        ));
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn apply_rejects_a_target_outside_the_repo() {
        let repo = tmp("apply-escape");
        let outside = std::env::temp_dir().join(format!("aoa-escape-{}.md", std::process::id()));
        let err = apply(&repo, &create_plan(outside.clone(), "# pwned\n")).unwrap_err();
        assert!(matches!(err, MigrateError::PathOutsideRepo { .. }));
        assert!(!outside.exists(), "nothing written outside the repo");
        assert!(!manifest_path(&repo).exists(), "no manifest on rejection");
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn apply_refuses_to_overwrite_an_existing_create_target() {
        let repo = tmp("apply-exists");
        let target = repo.join("README.md");
        fs::write(&target, "DO NOT CLOBBER\n").unwrap();

        let err = apply(&repo, &create_plan(target.clone(), "# new\n")).unwrap_err();
        assert!(matches!(err, MigrateError::TargetExists { .. }));
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "DO NOT CLOBBER\n",
            "existing content must be untouched"
        );
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn rollback_removes_created_files_and_bookkeeping() {
        let repo = tmp("rollback-create");
        let target = repo.join("README.md");
        apply(&repo, &create_plan(target.clone(), "# hi\n")).unwrap();
        assert!(target.exists());

        rollback(&repo).unwrap();
        assert!(!target.exists(), "created file removed on rollback");
        assert!(
            !repo.join(MIGRATE_DIR).exists(),
            "migration bookkeeping cleaned up"
        );
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn overwrite_archives_original_and_rollback_restores_it() {
        let repo = tmp("rollback-modify");
        let target = repo.join("config.txt");
        fs::write(&target, "ORIGINAL\n").unwrap();

        let plan = MigrationPlan {
            changes: vec![PlannedChange {
                path: target.clone(),
                action: ChangeAction::Overwrite,
                new_content: "REPLACED\n".to_string(),
                old_content: Some("ORIGINAL\n".to_string()),
            }],
            fix_ids: vec!["test-overwrite".to_string()],
            eligibility_notes: Vec::new(),
        };
        apply(&repo, &plan).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "REPLACED\n");

        rollback(&repo).unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "ORIGINAL\n",
            "original restored from archive"
        );
        fs::remove_dir_all(&repo).ok();
    }

    fn overwrite_change(path: PathBuf, old: &str, new: &str) -> PlannedChange {
        PlannedChange {
            path,
            action: ChangeAction::Overwrite,
            new_content: new.to_string(),
            old_content: Some(old.to_string()),
        }
    }

    #[test]
    fn multi_file_overwrite_archives_and_rollback_restores_all() {
        let repo = tmp("overwrite-multi");
        let a = repo.join("a.txt");
        let b = repo.join("b.txt");
        fs::write(&a, "A-ORIG\n").unwrap();
        fs::write(&b, "B-ORIG\n").unwrap();

        let plan = MigrationPlan {
            changes: vec![
                overwrite_change(a.clone(), "A-ORIG\n", "A-NEW\n"),
                overwrite_change(b.clone(), "B-ORIG\n", "B-NEW\n"),
            ],
            fix_ids: vec!["multi".to_string()],
            eligibility_notes: Vec::new(),
        };
        apply(&repo, &plan).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "A-NEW\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B-NEW\n");

        rollback(&repo).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "A-ORIG\n", "a restored");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B-ORIG\n", "b restored");
        assert!(!repo.join(MIGRATE_DIR).exists());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn rollback_after_partial_overwrite_restores_archived_and_leaves_untouched() {
        // Crash mid-apply across two Modified entries: entry A was archived then
        // replaced; entry B's write never happened, so its archive is absent and
        // its original is still intact. Rollback must restore A from its archive
        // and leave B alone (a missing archive means the write never landed).
        let repo = tmp("overwrite-partial");
        let a = repo.join("a.txt");
        let b = repo.join("b.txt");
        let archive_a = repo.join(MIGRATE_DIR).join(ARCHIVE_DIR).join("a.txt");
        let archive_b = repo.join(MIGRATE_DIR).join(ARCHIVE_DIR).join("b.txt");
        fs::write(&b, "B-ORIG\n").unwrap(); // B untouched by the partial apply

        let manifest = MigrationManifest {
            fixes_applied: vec!["partial-overwrite".to_string()],
            entries: vec![
                ManifestEntry::Modified {
                    path: a.clone(),
                    archive: archive_a.clone(),
                },
                ManifestEntry::Modified {
                    path: b.clone(),
                    archive: archive_b.clone(),
                },
            ],
            eligibility_notes: Vec::new(),
        };
        create_dir_all(&repo.join(MIGRATE_DIR)).unwrap();
        write_manifest(&repo, &manifest).unwrap();
        // Simulate A having been archived-then-replaced before the crash.
        create_dir_all(archive_a.parent().unwrap()).unwrap();
        fs::write(&archive_a, "A-ORIG\n").unwrap();
        fs::write(&a, "A-NEW\n").unwrap();
        // B's archive deliberately absent (write never reached it).

        rollback(&repo).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "A-ORIG\n", "A restored");
        assert_eq!(
            fs::read_to_string(&b).unwrap(),
            "B-ORIG\n",
            "B left intact (archive absent => write never landed)"
        );
        assert!(!repo.join(MIGRATE_DIR).exists());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn rollback_after_partial_apply_leaves_tree_at_baseline() {
        // Simulate a crash mid-apply: the manifest lists two creates but only one
        // file got written. Rollback (driven by the plan-first manifest) must
        // delete both planned targets, restoring the empty baseline.
        let repo = tmp("rollback-partial");
        let a = repo.join("A.md");
        let b = repo.join("B.md");
        let manifest = MigrationManifest {
            fixes_applied: vec!["partial".to_string()],
            entries: vec![
                ManifestEntry::Created { path: a.clone() },
                ManifestEntry::Created { path: b.clone() },
            ],
            eligibility_notes: Vec::new(),
        };
        create_dir_all(&repo.join(MIGRATE_DIR)).unwrap();
        write_manifest(&repo, &manifest).unwrap();
        // Only A landed before the "crash".
        fs::write(&a, "partial\n").unwrap();

        rollback(&repo).unwrap();
        assert!(!a.exists(), "landed file removed");
        assert!(
            !b.exists(),
            "never-written file is a no-op delete, not an error"
        );
        assert!(!repo.join(MIGRATE_DIR).exists());
        fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn manifest_round_trips_through_disk() {
        let repo = tmp("manifest-roundtrip");
        let target = repo.join("README.md");
        let applied = apply(&repo, &create_plan(target, "# hi\n")).unwrap();
        let read = read_manifest(&repo).unwrap();
        assert_eq!(applied, read);
        fs::remove_dir_all(&repo).ok();
    }
}
