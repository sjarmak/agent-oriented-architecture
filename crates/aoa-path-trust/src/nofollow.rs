use std::path::{Component, Path, PathBuf};

use super::{validate_single_component, PathTrustError};

/// Is `node` a symlink, without following it? The single spelling of that
/// question in the workspace — every check-then-act symlink guard goes here.
///
/// Deliberately not [`Path::is_symlink`]: that helper folds *every* lstat
/// failure into `false`, so a guard built on it fails OPEN on
/// `EACCES`/`ENOTDIR`/`ELOOP`. Here, only `NotFound` means "absent, safe to
/// create"; any other error surfaces as [`PathTrustError::Io`].
pub fn is_symlink_nofollow(node: &Path) -> Result<bool, PathTrustError> {
    match std::fs::symlink_metadata(node) {
        Ok(meta) => Ok(meta.file_type().is_symlink()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PathTrustError::io(node, source)),
    }
}

/// Refuse a single node that already exists as a symlink.
pub fn reject_symlink(node: &Path) -> Result<(), PathTrustError> {
    if is_symlink_nofollow(node)? {
        return Err(PathTrustError::unsafe_path(node));
    }
    Ok(())
}

/// Join a relative path beneath `root`, rejecting illegal components and every
/// existing symlink encountered from the root downward.
///
/// This is the shared check-then-act fallback for write paths that cannot use
/// descriptor-relative `openat` (see [`super::dirfd`] on Unix). A missing
/// component is safe to create; every other metadata failure is surfaced
/// instead of being treated as absent.
///
/// Each node is lstat'd in its own right, so the check does not depend on visit
/// order: lstat of `a/b` through a symlinked `a` reports the *target's* real
/// node and passes, but the separate lstat of `a` still catches it.
///
/// Symlinks at or above `root` remain allowed: `root` is the caller's chosen
/// trust root, not an escape from it.
pub fn safe_join_nofollow(root: &Path, relative: &Path) -> Result<PathBuf, PathTrustError> {
    let mut resolved = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(PathTrustError::unsafe_path(root.join(relative)));
        };
        let Some(part_name) = part.to_str() else {
            return Err(PathTrustError::unsafe_path(root.join(relative)));
        };
        validate_single_component(part_name)
            .map_err(|_| PathTrustError::unsafe_path(root.join(relative)))?;
        resolved.push(part);
        reject_symlink(&resolved)?;
    }
    Ok(resolved)
}

pub fn read_regular_file_nofollow(
    directory: &Path,
    name: &str,
) -> Result<Option<String>, PathTrustError> {
    use std::io::Read;

    let path = directory.join(name);
    let Some(mut file) = open_regular_file_nofollow(directory, name, &path)? else {
        return Ok(None);
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|source| PathTrustError::io(&path, source))?;
    Ok(Some(contents))
}

#[cfg(unix)]
fn open_regular_file_nofollow(
    directory: &Path,
    name: &str,
    path: &Path,
) -> Result<Option<std::fs::File>, PathTrustError> {
    let parent = super::dirfd::open_trust_root(directory)?;
    super::dirfd::open_regular_file_at(&parent, name, path)
}

#[cfg(not(unix))]
fn open_regular_file_nofollow(
    _directory: &Path,
    _name: &str,
    path: &Path,
) -> Result<Option<std::fs::File>, PathTrustError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(PathTrustError::io(path, source)),
    };
    if metadata.file_type().is_symlink() {
        return Err(PathTrustError::unsafe_path(path));
    }
    if !metadata.is_file() {
        return Err(PathTrustError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    std::fs::File::open(path)
        .map(Some)
        .map_err(|source| PathTrustError::io(path, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_regular_file_and_reports_an_absent_one_as_none() {
        let root = tempfile::tempdir().expect("create root");
        assert!(read_regular_file_nofollow(root.path(), "policy.yaml")
            .expect("an absent entry is not a failure")
            .is_none());

        std::fs::write(root.path().join("policy.yaml"), "contents\n").expect("write file");
        assert_eq!(
            read_regular_file_nofollow(root.path(), "policy.yaml")
                .expect("read regular file")
                .as_deref(),
            Some("contents\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_file_whether_or_not_its_target_exists() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let outside = tempfile::tempdir().expect("create outside");
        let real = outside.path().join("real.yaml");
        std::fs::write(&real, "contents\n").expect("write link target");
        symlink(&real, root.path().join("live.yaml")).expect("plant live link");
        symlink(
            outside.path().join("absent.yaml"),
            root.path().join("dangling.yaml"),
        )
        .expect("plant dangling link");

        for name in ["live.yaml", "dangling.yaml"] {
            let result = read_regular_file_nofollow(root.path(), name);
            assert!(
                matches!(
                    &result,
                    Err(PathTrustError::UnsafePath { path }) if path == &root.path().join(name)
                ),
                "{name} must be refused, not followed or read as absent: {result:?}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&real).expect("read link target"),
            "contents\n"
        );
    }

    #[test]
    fn refuses_an_entry_that_is_not_a_regular_file() {
        let root = tempfile::tempdir().expect("create root");
        std::fs::create_dir(root.path().join("policy.yaml")).expect("create dir");

        let result = read_regular_file_nofollow(root.path(), "policy.yaml");
        assert!(
            matches!(result, Err(PathTrustError::NotRegularFile { .. })),
            "a directory squatting the name must not read as absent: {result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_fails_closed_rather_than_reading_as_absent() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("create root");
        let sealed = root.path().join("policy.yaml");
        std::fs::write(&sealed, "contents\n").expect("write file");
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000))
            .expect("seal file");

        if std::fs::File::open(&sealed).is_err() {
            let result = read_regular_file_nofollow(root.path(), "policy.yaml");
            assert!(
                matches!(result, Err(PathTrustError::Io { .. })),
                "an unreadable file must not read as absent: {result:?}"
            );
        }
    }

    #[test]
    fn joins_only_relative_normal_components() {
        let root = tempfile::tempdir().expect("create root");
        let target = safe_join_nofollow(root.path(), Path::new("nested/file"))
            .expect("missing normal components are safe to create");
        assert_eq!(target, root.path().join("nested/file"));

        for relative in [
            Path::new("../escape"),
            Path::new("/absolute"),
            Path::new("back\\slash"),
        ] {
            assert!(matches!(
                safe_join_nofollow(root.path(), relative),
                Err(PathTrustError::UnsafePath { .. })
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_intermediate_component() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let outside = tempfile::tempdir().expect("create outside");
        symlink(outside.path(), root.path().join("nested")).expect("plant link");

        let err = safe_join_nofollow(root.path(), Path::new("nested/file"))
            .expect_err("walker must reject the planted link");
        assert!(matches!(
            err,
            PathTrustError::UnsafePath { path } if path == root.path().join("nested")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_node_fails_closed_rather_than_reading_as_absent() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("create root");
        let sealed = root.path().join("sealed");
        std::fs::create_dir(&sealed).expect("create sealed dir");
        std::fs::create_dir(sealed.join("inner")).expect("create inner dir");
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000))
            .expect("seal dir");

        let result = safe_join_nofollow(root.path(), Path::new("sealed/inner/file"));
        let sealed_took = std::fs::symlink_metadata(sealed.join("inner")).is_err();

        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o700))
            .expect("restore dir");

        if !sealed_took {
            eprintln!(
                "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process searches \
                 a mode-000 directory, so it cannot be denied the lstat"
            );
            return;
        }
        assert!(
            matches!(result, Err(PathTrustError::Io { .. })),
            "an inconclusive lstat must not read as absent: {result:?}"
        );
    }
}
