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

#[cfg(test)]
mod tests {
    use super::*;

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
        // Running as root defeats the permission bits entirely; only assert the
        // fail-closed contract when the seal actually took.
        let sealed_took = std::fs::symlink_metadata(sealed.join("inner")).is_err();

        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o700))
            .expect("restore dir");

        if sealed_took {
            assert!(
                matches!(result, Err(PathTrustError::Io { .. })),
                "an inconclusive lstat must not read as absent: {result:?}"
            );
        }
    }
}
