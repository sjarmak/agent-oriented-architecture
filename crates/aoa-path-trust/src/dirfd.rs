//! Descriptor-relative directory acquisition.
//!
//! The Unix half of the path-trust boundary. Where [`super::nofollow`] can only
//! check-then-act, these primitives acquire each directory with
//! `openat(O_NOFOLLOW | O_DIRECTORY)` and let the caller mutate relative to the
//! returned descriptor, so a path substituted after the check cannot redirect
//! the write.

use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::{validate_single_component, PathTrustError};

/// Mode for directories this module creates: owner-only.
const DIR_MODE: Mode = Mode::RWXU;

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Is `name` inside the directory `parent` refers to a symlink? Used to explain
/// an `openat` refusal, never as an authorization check in its own right.
pub fn is_symlink_at(parent: impl AsFd, name: impl AsRef<OsStr>) -> bool {
    fs::statat(parent, name.as_ref(), AtFlags::SYMLINK_NOFOLLOW)
        .map(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
        .unwrap_or(false)
}

/// Classify an `O_NOFOLLOW` open failure: a link in the way is a trust refusal,
/// anything else is a plain IO failure.
pub fn map_nofollow_error(
    parent: impl AsFd,
    name: impl AsRef<OsStr>,
    path: &Path,
    source: std::io::Error,
) -> PathTrustError {
    if source.raw_os_error() == Some(Errno::LOOP.raw_os_error()) || is_symlink_at(parent, name) {
        PathTrustError::symlink(path)
    } else {
        PathTrustError::io(path, source)
    }
}

/// Open the caller-selected trust root.
///
/// Symlinks above or at that root remain allowed: it is the caller's choice of
/// root. Every component acquired beneath it goes through [`open_dir_at`] or
/// [`open_or_create_dir_at`] relative to this descriptor with `O_NOFOLLOW`.
pub fn open_trust_root(root: &Path) -> Result<OwnedFd, PathTrustError> {
    fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| PathTrustError::io(root, source.into()))
}

fn single_component(name: &OsStr, path: &Path) -> Result<(), PathTrustError> {
    let accepted = name
        .to_str()
        .is_some_and(|name| validate_single_component(name).is_ok());
    if accepted {
        Ok(())
    } else {
        Err(PathTrustError::unsafe_component(path))
    }
}

/// Acquire an existing subdirectory `name` of `parent`, refusing a symlink.
pub fn open_dir_at(
    parent: impl AsFd,
    name: impl AsRef<OsStr>,
    path: &Path,
) -> Result<OwnedFd, PathTrustError> {
    let name = name.as_ref();
    single_component(name, path)?;
    fs::openat(parent.as_fd(), name, DIRECTORY_FLAGS, Mode::empty())
        .map_err(|source| map_nofollow_error(parent, name, path, source.into()))
}

/// Acquire subdirectory `name` of `parent`, creating it when absent. A racing
/// creator is tolerated (`EEXIST`), a symlink in the way is not.
pub fn open_or_create_dir_at(
    parent: impl AsFd,
    name: impl AsRef<OsStr>,
    path: &Path,
) -> Result<OwnedFd, PathTrustError> {
    let name = name.as_ref();
    single_component(name, path)?;
    match fs::openat(parent.as_fd(), name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(fd),
        Err(Errno::NOENT) => {
            match fs::mkdirat(parent.as_fd(), name, DIR_MODE) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(source) => return Err(PathTrustError::io(path, source.into())),
            }
            open_dir_at(parent, name, path)
        }
        Err(source) => Err(map_nofollow_error(parent, name, path, source.into())),
    }
}

pub fn open_regular_file_at(
    parent: impl AsFd,
    name: &str,
    path: &Path,
) -> Result<Option<File>, PathTrustError> {
    single_component(OsStr::new(name), path)?;
    let fd = match fs::openat(
        parent.as_fd(),
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Ok(None),
        Err(source) => return Err(map_nofollow_error(parent, name, path, source.into())),
    };
    let stat = fs::fstat(&fd).map_err(|source| PathTrustError::io(path, source.into()))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(PathTrustError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    Ok(Some(File::from(fd)))
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
            assert!(matches!(result, Err(PathTrustError::Symlink { .. })));
        }
        assert_eq!(
            std::fs::read_dir(outside.path())
                .expect("read outside")
                .count(),
            0,
            "the planted link's target must not have been touched"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn refuses_a_non_utf8_name_before_opening_even_when_it_is_a_link() {
        use std::os::unix::ffi::OsStrExt;

        let root = tempfile::tempdir().expect("create root");
        let outside = tempfile::tempdir().expect("create outside");
        let name = OsStr::from_bytes(b"link-\xff");
        let path = root.path().join(name);
        symlink(outside.path(), &path).expect("plant link");
        let root_fd = open_trust_root(root.path()).expect("open trust root");

        assert!(is_symlink_at(&root_fd, name));
        for result in [
            open_dir_at(&root_fd, name, &path),
            open_or_create_dir_at(&root_fd, name, &path),
        ] {
            assert!(matches!(
                result,
                Err(PathTrustError::UnsafeComponent { .. })
            ));
        }
        assert_eq!(
            std::fs::read_dir(outside.path())
                .expect("read outside")
                .count(),
            0,
            "the planted link's target must not have been touched"
        );
    }

    #[test]
    fn acquires_directories_only_by_a_single_component_name() {
        let base = tempfile::tempdir().expect("create base");
        let outside = tempfile::tempdir().expect("create outside");
        let root = base.path().join("root");
        std::fs::create_dir(&root).expect("create root");
        symlink(outside.path(), root.join("linked")).expect("plant link");
        let root_fd = open_trust_root(&root).expect("open trust root");

        for name in ["linked/inner", "../escaped", "./inner", ".", "..", ""] {
            let path = root.join(name);
            for result in [
                open_dir_at(&root_fd, name, &path),
                open_or_create_dir_at(&root_fd, name, &path),
            ] {
                assert!(
                    matches!(
                        &result,
                        Err(PathTrustError::UnsafeComponent { path: refused }) if refused == &path
                    ),
                    "{name:?} must be refused before anything is opened: {result:?}"
                );
            }
        }
        assert_eq!(
            std::fs::read_dir(outside.path())
                .expect("read outside")
                .count(),
            0,
            "the planted link's target must not have been touched"
        );
        let beside_root: Vec<_> = std::fs::read_dir(base.path())
            .expect("read base")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(beside_root, ["root"], "a parent step escaped the root");
    }

    #[test]
    fn every_acquired_descriptor_is_closed_on_exec() {
        use rustix::io::{fcntl_getfd, FdFlags};

        let root = tempfile::tempdir().expect("create root");
        let own = root.path().join("policy.yaml");
        std::fs::write(&own, "own\n").expect("write own file");
        let root_fd = open_trust_root(root.path()).expect("open trust root");
        let made = root.path().join("made");
        let created = open_or_create_dir_at(&root_fd, "made", &made).expect("create dir");
        let reacquired = open_dir_at(&root_fd, "made", &made).expect("reacquire dir");
        let file = open_regular_file_at(&root_fd, "policy.yaml", &own)
            .expect("open own file")
            .expect("own file exists");

        for (what, fd) in [
            ("trust root", root_fd.as_fd()),
            ("created directory", created.as_fd()),
            ("reacquired directory", reacquired.as_fd()),
            ("regular file", file.as_fd()),
        ] {
            let flags = fcntl_getfd(fd).expect("read descriptor flags");
            assert!(
                flags.contains(FdFlags::CLOEXEC),
                "the {what} descriptor would leak into a child process"
            );
        }
    }

    #[test]
    fn opens_a_regular_file_only_by_a_single_component_name() {
        let root = tempfile::tempdir().expect("create root");
        let outside = tempfile::tempdir().expect("create outside");
        std::fs::write(outside.path().join("policy.yaml"), "outside\n")
            .expect("plant outside file");
        symlink(outside.path(), root.path().join("linked")).expect("plant link");
        std::fs::write(root.path().join("policy.yaml"), "own\n").expect("write own file");
        let root_fd = open_trust_root(root.path()).expect("open trust root");

        for name in ["linked/policy.yaml", "./policy.yaml", "..", ".", ""] {
            let path = root.path().join(name);
            let result = open_regular_file_at(&root_fd, name, &path);
            assert!(
                matches!(
                    &result,
                    Err(PathTrustError::UnsafeComponent { path: refused }) if refused == &path
                ),
                "{name:?} must be refused before anything is opened: {result:?}"
            );
        }
        assert!(
            open_regular_file_at(&root_fd, "policy.yaml", &root.path().join("policy.yaml"))
                .expect("open own file")
                .is_some()
        );
    }
}
