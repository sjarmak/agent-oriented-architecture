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
    let (mode, probe) = match path.is_dir() {
        true => (0o555, path.join("probe")),
        false => (0o444, path.to_path_buf()),
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&probe)
        .is_ok();
    if written {
        eprintln!(
            "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process writes \
             past a mode that denies it, so it cannot be denied a write"
        );
    }
    !written
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
            Err(BudgetError::RootNotWritable { path, source })
                if path == &root && source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "{refused:?}"
    );
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("may not write"), "{message}");
    assert!(!message.contains("failed to read"), "{message}");
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(mode(&root), 0o444);
    assert_eq!(names_in(dir.path()), ["big.md"]);
}

#[test]
fn fix_refuses_an_archive_the_process_may_not_create_before_it_touches_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let root = dir.path().join("big.md");
    std::fs::write(&root, &body).unwrap();
    let archive = dir.path().join("big.archive.md");
    if !seal_against_writing(dir.path()) {
        return;
    }

    let refused = fix_oversized(&root, dir.path(), 200, "gpt-4o");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        matches!(
            &refused,
            Err(BudgetError::Io { path, source })
                if path == &archive && source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(names_in(dir.path()), ["big.md"]);
}

#[test]
fn fix_changes_nothing_when_the_file_behind_a_linked_root_cannot_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let held = dir.path().join("held");
    std::fs::create_dir(&held).unwrap();
    std::fs::write(held.join("real.md"), &body).unwrap();
    let root = dir.path().join("big.md");
    symlink("held/real.md", &root).unwrap();
    if !seal_against_writing(&held) {
        return;
    }

    let refused = fix_oversized(&root, dir.path(), 200, "gpt-4o");
    std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        matches!(
            &refused,
            Err(BudgetError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(held.join("real.md")).unwrap(), body);
    assert_eq!(names_in(dir.path()), ["big.md", "held"]);
    assert_eq!(names_in(&held), ["real.md"]);
}

#[cfg(target_os = "linux")]
#[test]
fn fix_carries_the_extended_attributes_of_the_root_it_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("big.md");
    std::fs::write(&root, oversized_body()).unwrap();
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
    let labelled = |path: &Path| {
        let mut value = [0u8; 16];
        let filled = rustix::fs::getxattr(path, "user.aoa.label", &mut value[..]).unwrap();
        value[..filled].to_vec()
    };

    fix_oversized(&root, dir.path(), 200, "gpt-4o").unwrap();

    assert_eq!(labelled(&root), b"root");
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
            Err(BudgetError::ArchiveExists { path, archive: named })
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
fn fix_archives_beside_the_file_a_root_reaches_through_a_directory_link_named_like_its_archive() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let body = oversized_body();
    std::fs::create_dir(base.join("held")).unwrap();
    let held = base.join("held/real.md");
    std::fs::write(&held, &body).unwrap();
    let directory_link = base.join("big.archive.md");
    symlink("held", &directory_link).unwrap();
    let root = base.join("big.md");
    symlink("big.archive.md/real.md", &root).unwrap();

    let fixed = fix_oversized(&root, &base, 200, "gpt-4o").unwrap();

    assert_eq!(fixed.root, root);
    assert_eq!(fixed.archive, base.join("held/big.archive.md"));
    assert_eq!(
        std::fs::read_to_string(base.join("held/big.archive.md")).unwrap(),
        body
    );
    assert!(std::fs::read_to_string(&held)
        .unwrap()
        .contains("[big.archive.md]"));
    assert_eq!(
        std::fs::read_link(&root).unwrap(),
        Path::new("big.archive.md/real.md")
    );
    assert_eq!(
        std::fs::read_link(&directory_link).unwrap(),
        Path::new("held")
    );
    assert_eq!(names_in(&base), ["big.archive.md", "big.md", "held"]);
    assert_eq!(names_in(&base.join("held")), ["big.archive.md", "real.md"]);
}
