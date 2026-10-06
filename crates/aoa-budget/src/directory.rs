use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io;
use std::path::PathBuf;

const FRESH_NAME_ATTEMPTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    Replacing,
    Creating,
}

pub(crate) struct Directory {
    path: PathBuf,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

impl Directory {
    pub(crate) fn naming(&self, entry: &OsStr) -> PathBuf {
        self.path.join(entry)
    }

    pub(crate) fn create_fresh(&self, prefix: &str) -> io::Result<(OsString, File)> {
        let mut taken = None;
        for _ in 0..FRESH_NAME_ATTEMPTS {
            let name = OsString::from(format!("{prefix}{:016x}", random()));
            match self.create_new(&name) {
                Ok(file) => return Ok((name, file)),
                Err(exists) if exists.kind() == io::ErrorKind::AlreadyExists => {
                    taken = Some(exists)
                }
                Err(error) => return Err(error),
            }
        }
        Err(taken.unwrap_or_else(|| io::ErrorKind::AlreadyExists.into()))
    }
}

fn random() -> u64 {
    RandomState::new().build_hasher().finish()
}

#[cfg(unix)]
impl Directory {
    pub(crate) fn hold(traversed: &std::os::fd::OwnedFd, path: PathBuf) -> io::Result<Self> {
        use rustix::fs::OFlags;

        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(traversed, ".", flags, rustix::fs::Mode::empty())?;
        Ok(Self { path, fd })
    }

    pub(crate) fn open_writable(&self, name: &OsStr) -> io::Result<File> {
        use rustix::fs::OFlags;

        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.fd, name, flags, rustix::fs::Mode::empty())?;
        Ok(File::from(fd))
    }

    fn create_new(&self, name: &OsStr) -> io::Result<File> {
        use rustix::fs::{Mode, OFlags};

        let flags =
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.fd, name, flags, Mode::RUSR | Mode::WUSR)?;
        Ok(File::from(fd))
    }

    pub(crate) fn holds(&self, name: &OsStr) -> io::Result<bool> {
        match rustix::fs::statat(&self.fd, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn place(&self, from: &OsStr, to: &OsStr, placement: Placement) -> io::Result<()> {
        use rustix::fs::AtFlags;

        match placement {
            Placement::Replacing => rustix::fs::renameat(&self.fd, from, &self.fd, to)?,
            Placement::Creating => {
                rustix::fs::linkat(&self.fd, from, &self.fd, to, AtFlags::empty())?;
                rustix::fs::unlinkat(&self.fd, from, AtFlags::empty())?;
            }
        }
        Ok(())
    }

    pub(crate) fn remove(&self, name: &OsStr) -> io::Result<()> {
        rustix::fs::unlinkat(&self.fd, name, rustix::fs::AtFlags::empty())?;
        Ok(())
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        rustix::fs::fsync(&self.fd)?;
        Ok(())
    }
}

#[cfg(not(unix))]
impl Directory {
    pub(crate) fn hold(path: PathBuf) -> io::Result<Self> {
        if !std::fs::metadata(&path)?.is_dir() {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        Ok(Self { path })
    }

    pub(crate) fn open_writable(&self, name: &OsStr) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.naming(name))
    }

    fn create_new(&self, name: &OsStr) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(self.naming(name))
    }

    pub(crate) fn holds(&self, name: &OsStr) -> io::Result<bool> {
        match std::fs::symlink_metadata(self.naming(name)) {
            Ok(_) => Ok(true),
            Err(absent) if absent.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn place(&self, from: &OsStr, to: &OsStr, placement: Placement) -> io::Result<()> {
        let (from, to) = (self.naming(from), self.naming(to));
        match placement {
            Placement::Replacing => std::fs::rename(from, to),
            Placement::Creating => {
                std::fs::hard_link(&from, to)?;
                std::fs::remove_file(from)
            }
        }
    }

    pub(crate) fn remove(&self, name: &OsStr) -> io::Result<()> {
        std::fs::remove_file(self.naming(name))
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        Ok(())
    }
}
