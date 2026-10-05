#![cfg(unix)]

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use aoa_budget::{fix_oversized, BudgetError};

fn oversized_body() -> String {
    "A paragraph of guidance that every package must follow.\n\n".repeat(200)
}

fn names_in(dir: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    names
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn seal_against_writing(path: &Path) -> bool {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new().write(true).open(path).is_ok() {
        eprintln!(
            "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process opens a \
             mode-444 file for writing, so it cannot be denied a write"
        );
        return false;
    }
    true
}

#[test]
fn fix_leaves_an_archive_that_already_stands_with_its_own_mode() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let root = dir.path().join("big.md");
    std::fs::write(&root, &body).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o644)).unwrap();
    let archive = dir.path().join("big.archive.md");
    std::fs::write(&archive, "an earlier archive\n").unwrap();
    std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o600)).unwrap();

    fix_oversized(&root, dir.path(), 200, "gpt-4o").unwrap();

    assert_eq!(std::fs::read_to_string(&archive).unwrap(), body);
    assert_eq!(mode(&archive), 0o600);
    assert_eq!(mode(&root), 0o644);
}

#[test]
fn fix_refuses_a_root_the_process_may_not_write_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let root = dir.path().join("big.md");
    std::fs::write(&root, &body).unwrap();
    if !seal_against_writing(&root) {
        return;
    }

    let refused = fix_oversized(&root, dir.path(), 200, "gpt-4o");

    assert!(
        matches!(
            &refused,
            Err(BudgetError::Io { path, source })
                if path == &root && source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(mode(&root), 0o444);
    assert_eq!(names_in(dir.path()), ["big.md"]);
}

#[test]
fn fix_refuses_an_archive_the_process_may_not_write_before_it_touches_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let root = dir.path().join("big.md");
    std::fs::write(&root, &body).unwrap();
    let archive = dir.path().join("big.archive.md");
    std::fs::write(&archive, "an earlier archive\n").unwrap();
    if !seal_against_writing(&archive) {
        return;
    }

    let refused = fix_oversized(&root, dir.path(), 200, "gpt-4o");

    assert!(
        matches!(
            &refused,
            Err(BudgetError::Io { path, source })
                if path == &archive && source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(
        std::fs::read_to_string(&archive).unwrap(),
        "an earlier archive\n"
    );
    assert_eq!(names_in(dir.path()), ["big.archive.md", "big.md"]);
}

#[cfg(target_os = "linux")]
#[test]
fn fix_carries_the_extended_attributes_of_each_file_it_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("big.md");
    std::fs::write(&root, oversized_body()).unwrap();
    let archive = dir.path().join("big.archive.md");
    std::fs::write(&archive, "an earlier archive\n").unwrap();
    let label = |path: &Path, value: &[u8]| {
        rustix::fs::setxattr(
            path,
            "user.aoa.label",
            value,
            rustix::fs::XattrFlags::empty(),
        )
    };
    if label(&root, b"root") == Err(rustix::io::Errno::NOTSUP) {
        eprintln!(
            "SKIP (docs/adr/0004-environment-dependent-test-skips.md): the temporary directory \
             is on a filesystem without user extended attributes"
        );
        return;
    }
    label(&archive, b"archive").unwrap();
    let labelled = |path: &Path| {
        let mut value = [0u8; 16];
        let filled = rustix::fs::getxattr(path, "user.aoa.label", &mut value[..]).unwrap();
        value[..filled].to_vec()
    };

    fix_oversized(&root, dir.path(), 200, "gpt-4o").unwrap();

    assert_eq!(labelled(&root), b"root");
    assert_eq!(labelled(&archive), b"archive");
}

#[test]
fn fix_refuses_a_root_whose_link_chain_passes_through_its_archive_name() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let other = dir.path().join("other.md");
    std::fs::write(&other, &body).unwrap();
    let archive = dir.path().join("big.archive.md");
    symlink("other.md", &archive).unwrap();
    let root = dir.path().join("big.md");
    symlink("big.archive.md", &root).unwrap();

    let refused = fix_oversized(&root, dir.path(), 200, "gpt-4o");

    assert!(
        matches!(
            &refused,
            Err(BudgetError::RootThroughArchive { path, archive: named })
                if path == &root && named == &archive
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&other).unwrap(), body);
    assert_eq!(std::fs::read_link(&archive).unwrap(), Path::new("other.md"));
    assert_eq!(
        std::fs::read_link(&root).unwrap(),
        Path::new("big.archive.md")
    );
    assert_eq!(
        names_in(dir.path()),
        ["big.archive.md", "big.md", "other.md"]
    );
}

#[test]
fn fix_refuses_a_root_that_steps_through_its_archive_name_as_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    std::fs::create_dir(dir.path().join("held")).unwrap();
    let held = dir.path().join("held/real.md");
    std::fs::write(&held, &body).unwrap();
    let archive = dir.path().join("big.archive.md");
    symlink("held", &archive).unwrap();
    let root = dir.path().join("big.md");
    symlink("big.archive.md/real.md", &root).unwrap();

    let refused = fix_oversized(&root, dir.path(), 200, "gpt-4o");

    assert!(
        matches!(&refused, Err(BudgetError::RootThroughArchive { path, .. }) if path == &root),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&held).unwrap(), body);
    assert_eq!(std::fs::read_link(&archive).unwrap(), Path::new("held"));
    assert_eq!(names_in(dir.path()), ["big.archive.md", "big.md", "held"]);
}
