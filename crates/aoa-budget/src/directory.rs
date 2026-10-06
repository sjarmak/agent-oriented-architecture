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
        match placement {
            Placement::Replacing => rustix::fs::renameat(&self.fd, from, &self.fd, to)?,
            Placement::Creating => self.create_name(from, to)?,
        }
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn create_name(&self, from: &OsStr, to: &OsStr) -> rustix::io::Result<()> {
        let flags = rustix::fs::RenameFlags::NOREPLACE;
        match rustix::fs::renameat_with(&self.fd, from, &self.fd, to, flags) {
            Err(refused) if lacks_noreplace(refused) => self.link_then_unlink(from, to),
            placed => placed,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn create_name(&self, from: &OsStr, to: &OsStr) -> rustix::io::Result<()> {
        self.link_then_unlink(from, to)
    }

    fn link_then_unlink(&self, from: &OsStr, to: &OsStr) -> rustix::io::Result<()> {
        use rustix::fs::AtFlags;

        rustix::fs::linkat(&self.fd, from, &self.fd, to, AtFlags::empty())?;
        rustix::fs::unlinkat(&self.fd, from, AtFlags::empty())
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

#[cfg(any(target_os = "linux", target_os = "android"))]
fn lacks_noreplace(refused: rustix::io::Errno) -> bool {
    matches!(refused, rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS)
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;
    use crate::boundary::Boundary;

    fn held(dir: &std::path::Path) -> Directory {
        let boundary = Boundary::open(dir).unwrap();
        boundary
            .hold_directory(&dir.canonicalize().unwrap())
            .unwrap()
    }

    fn names_in(dir: &std::path::Path) -> Vec<OsString> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_name_created_from_a_fresh_file_leaves_one_link_and_no_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        let directory = held(dir.path());
        let (fresh, mut file) = directory.create_fresh(".temp-").unwrap();
        std::io::Write::write_all(&mut file, b"archived").unwrap();
        drop(file);
        let archive = dir.path().join("big.archive.md");

        directory
            .place(&fresh, OsStr::new("big.archive.md"), Placement::Creating)
            .unwrap();

        assert_eq!(std::fs::metadata(&archive).unwrap().nlink(), 1);
        assert_eq!(std::fs::read_to_string(&archive).unwrap(), "archived");
        assert_eq!(names_in(dir.path()), ["big.archive.md"]);
        assert!(!directory.holds(&fresh).unwrap());
    }

    #[test]
    fn a_name_created_over_a_taken_name_is_refused_and_both_entries_stay() {
        let dir = tempfile::tempdir().unwrap();
        let directory = held(dir.path());
        let (fresh, _file) = directory.create_fresh(".temp-").unwrap();
        let taken = dir.path().join("big.archive.md");
        std::fs::write(&taken, "planted").unwrap();

        let refused = directory
            .place(&fresh, OsStr::new("big.archive.md"), Placement::Creating)
            .unwrap_err();

        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "planted");
        assert_eq!(std::fs::metadata(&taken).unwrap().nlink(), 1);
        assert!(directory.holds(&fresh).unwrap());
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn only_an_unsupported_flag_falls_back_to_linking() {
        use rustix::io::Errno;

        assert!(lacks_noreplace(Errno::INVAL));
        assert!(lacks_noreplace(Errno::NOSYS));
        assert!(!lacks_noreplace(Errno::EXIST));
        assert!(!lacks_noreplace(Errno::PERM));
        assert!(!lacks_noreplace(Errno::XDEV));
    }
}
