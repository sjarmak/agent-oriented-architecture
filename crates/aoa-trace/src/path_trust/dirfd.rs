//! Descriptor-relative directory acquisition.
//!
//! The Unix half of the path-trust boundary. Where [`super::nofollow`] can only
//! check-then-act, these primitives acquire each directory with
//! `openat(O_NOFOLLOW | O_DIRECTORY)` and let the caller mutate relative to the
//! returned descriptor, so a path substituted after the check cannot redirect
//! the write.
//!
//! [`map_nofollow_error`] takes a `rustix::io::Errno`, so this module's surface
//! is coupled to the `rustix` major version a consumer resolves: a caller that
//! opens its own file relative to a descriptor acquired here must match on the
//! same `Errno` type. Consumers therefore track this crate's `rustix`
//! requirement rather than picking one independently.

use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::PathTrustError;

/// Mode for directories this module creates: owner-only.
const DIR_MODE: Mode = Mode::RWXU;

/// Is `name` inside the directory `parent` refers to a symlink? Used to explain
/// an `openat` refusal, never as an authorization check in its own right.
pub fn is_symlink_at(parent: impl AsFd, name: &str) -> bool {
    fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .map(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
        .unwrap_or(false)
}

/// Classify an `O_NOFOLLOW` open failure: a link in the way is a trust refusal,
/// anything else is a plain IO failure.
pub fn map_nofollow_error(
    parent: impl AsFd,
    name: &str,
    path: &Path,
    source: Errno,
) -> PathTrustError {
    if source == Errno::LOOP || is_symlink_at(parent, name) {
        PathTrustError::unsafe_path(path)
    } else {
        PathTrustError::io(path, source.into())
    }
}

/// Open the caller-selected trust root.
///
/// Symlinks above or at that root remain allowed: it is the caller's choice of
/// root. Every component acquired beneath it goes through [`open_dir_at`] or
/// [`open_or_create_dir_at`] relative to this descriptor with `O_NOFOLLOW`.
pub fn open_trust_root(root: &Path) -> Result<OwnedFd, PathTrustError> {
    fs::open(root, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())
        .map_err(|source| PathTrustError::io(root, source.into()))
}

/// Acquire an existing subdirectory `name` of `parent`, refusing a symlink.
pub fn open_dir_at(parent: impl AsFd, name: &str, path: &Path) -> Result<OwnedFd, PathTrustError> {
    fs::openat(
        parent.as_fd(),
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|source| map_nofollow_error(parent, name, path, source))
}

/// Acquire subdirectory `name` of `parent`, creating it when absent. A racing
/// creator is tolerated (`EEXIST`), a symlink in the way is not.
pub fn open_or_create_dir_at(
    parent: impl AsFd,
    name: &str,
    path: &Path,
) -> Result<OwnedFd, PathTrustError> {
    match fs::openat(
        parent.as_fd(),
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(fd),
        Err(Errno::NOENT) => {
            match fs::mkdirat(parent.as_fd(), name, DIR_MODE) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(source) => return Err(PathTrustError::io(path, source.into())),
            }
            open_dir_at(parent, name, path)
        }
        Err(source) => Err(map_nofollow_error(parent, name, path, source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn creates_then_reacquires_the_same_directory() {
        let root = tempfile::tempdir().expect("create root");
        let root_fd = open_trust_root(root.path()).expect("open trust root");
        let path = root.path().join("made");

        open_or_create_dir_at(&root_fd, "made", &path).expect("create dir");
        assert!(path.is_dir());
        open_dir_at(&root_fd, "made", &path).expect("reacquire existing dir");
    }

    #[test]
    fn refuses_a_symlinked_component_instead_of_following_it() {
        let root = tempfile::tempdir().expect("create root");
        let outside = tempfile::tempdir().expect("create outside");
        symlink(outside.path(), root.path().join("linked")).expect("plant link");
        let root_fd = open_trust_root(root.path()).expect("open trust root");
        let path = root.path().join("linked");

        for result in [
            open_dir_at(&root_fd, "linked", &path),
            open_or_create_dir_at(&root_fd, "linked", &path),
        ] {
            assert!(matches!(result, Err(PathTrustError::UnsafePath { .. })));
        }
        assert_eq!(
            std::fs::read_dir(outside.path())
                .expect("read outside")
                .count(),
            0,
            "the planted link's target must not have been touched"
        );
    }
}
