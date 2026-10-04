use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::escape::leaves_boundary;

pub(crate) struct Boundary {
    path: PathBuf,
    #[cfg(unix)]
    directory: std::os::fd::OwnedFd,
}

impl Boundary {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Self::at(path.canonicalize()?)
    }

    fn at(path: PathBuf) -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            directory: open_directory(&path)?,
            path,
        })
    }

    pub(crate) fn contains(&self, resolved: &Path) -> bool {
        resolved.starts_with(&self.path)
    }

    pub(crate) fn leaves(&self, target: &Path) -> bool {
        leaves_boundary(target, &self.path)
    }

    fn beneath<'a>(&self, resolved: &'a Path) -> io::Result<&'a Path> {
        resolved.strip_prefix(&self.path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "member is not beneath the boundary",
            )
        })
    }

    #[cfg(unix)]
    pub(crate) fn open_member(&self, resolved: &Path) -> io::Result<File> {
        let mut current = self.directory.try_clone()?;
        let mut names = self.beneath(resolved)?.components().peekable();
        while let Some(component) = names.next() {
            let std::path::Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "member path is not canonical",
                ));
            };
            let flags = if names.peek().is_some() {
                descend::DIRECTORY | rustix::fs::OFlags::NOFOLLOW
            } else {
                descend::MEMBER
            };
            current = rustix::fs::openat(&current, name, flags, rustix::fs::Mode::empty())?;
        }
        Ok(File::from(current))
    }

    #[cfg(not(unix))]
    pub(crate) fn open_member(&self, resolved: &Path) -> io::Result<File> {
        self.beneath(resolved)?;
        File::open(resolved)
    }
}

#[cfg(unix)]
fn open_directory(canonical: &Path) -> io::Result<std::os::fd::OwnedFd> {
    use std::path::Component;

    let not_canonical = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "boundary path is not canonical",
        )
    };
    let mut components = canonical.components();
    let Some(Component::RootDir) = components.next() else {
        return Err(not_canonical());
    };
    let mut current = rustix::fs::open("/", descend::DIRECTORY, rustix::fs::Mode::empty())?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(not_canonical());
        };
        current = rustix::fs::openat(
            &current,
            name,
            descend::DIRECTORY | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )?;
    }
    Ok(current)
}

#[cfg(unix)]
pub(crate) fn open_following_links(path: &Path) -> io::Result<File> {
    use rustix::fs::OFlags;

    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
    Ok(File::from(rustix::fs::open(
        path,
        flags,
        rustix::fs::Mode::empty(),
    )?))
}

#[cfg(not(unix))]
pub(crate) fn open_following_links(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(unix)]
mod descend {
    use rustix::fs::OFlags;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    const TRAVERSE: OFlags = OFlags::PATH;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const TRAVERSE: OFlags = OFlags::RDONLY;

    pub(super) const DIRECTORY: OFlags = TRAVERSE.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC);
    pub(super) const MEMBER: OFlags = OFlags::RDONLY
        .union(OFlags::NONBLOCK)
        .union(OFlags::NOCTTY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Read;

    use super::Boundary;

    #[test]
    fn a_directory_reached_without_links_opens_and_serves_its_members() {
        let dir = tempfile::tempdir().expect("tempdir");
        let docs = dir.path().canonicalize().expect("canonical").join("docs");
        std::fs::create_dir(&docs).expect("create docs");
        std::fs::write(docs.join("member.md"), "inside\n").expect("write member");

        let boundary = Boundary::at(docs.clone()).expect("open the boundary");
        let mut text = String::new();
        boundary
            .open_member(&docs.join("member.md"))
            .expect("open the member")
            .read_to_string(&mut text)
            .expect("read the member");

        assert_eq!(text, "inside\n");
    }

    #[test]
    fn a_directory_swapped_for_a_link_after_it_was_resolved_does_not_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir_all(base.join("outside/nested")).expect("create outside");
        std::fs::create_dir(base.join("repo")).expect("create repo");
        std::os::unix::fs::symlink(base.join("outside"), base.join("repo/docs"))
            .expect("swap docs for a link");
        assert!(base.join("repo/docs/nested").is_dir());

        assert!(Boundary::at(base.join("repo/docs")).is_err());
        assert!(Boundary::at(base.join("repo/docs/nested")).is_err());
    }

    #[test]
    fn a_member_whose_directory_is_swapped_for_a_link_after_the_boundary_opened_does_not_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir(base.join("outside")).expect("create outside");
        std::fs::write(base.join("outside/member.md"), "outside\n").expect("write outside member");
        std::fs::create_dir(base.join("repo")).expect("create repo");
        let boundary = Boundary::at(base.join("repo")).expect("open the boundary");
        std::os::unix::fs::symlink(base.join("outside"), base.join("repo/docs"))
            .expect("swap docs for a link");
        assert!(base.join("repo/docs/member.md").is_file());

        assert!(boundary
            .open_member(&base.join("repo/docs/member.md"))
            .is_err());
    }

    #[test]
    fn a_relative_directory_does_not_open() {
        assert!(Boundary::at(std::path::PathBuf::from("docs")).is_err());
    }
}
