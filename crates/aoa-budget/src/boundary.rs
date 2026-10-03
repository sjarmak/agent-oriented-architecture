use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct Boundary {
    path: PathBuf,
    #[cfg(unix)]
    directory: std::os::fd::OwnedFd,
}

impl Boundary {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        Ok(Self {
            #[cfg(unix)]
            directory: rustix::fs::open(&path, descend::DIRECTORY, rustix::fs::Mode::empty())?,
            path,
        })
    }

    pub(crate) fn contains(&self, resolved: &Path) -> bool {
        resolved.starts_with(&self.path)
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
