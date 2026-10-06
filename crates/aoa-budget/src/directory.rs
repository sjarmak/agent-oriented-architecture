use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};

const FRESH_NAME_ATTEMPTS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    Other,
}

#[derive(Debug)]
pub enum Opened {
    File(File),
    Absent,
    Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    Replacing,
    Creating,
}

pub struct Directory {
    path: PathBuf,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

impl Directory {
    pub fn path(&self) -> &Path {
        &self.path
    }

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

    pub fn open_member(&self, name: &OsStr) -> io::Result<File> {
        let flags = crate::boundary::descend::MEMBER;
        let fd = rustix::fs::openat(&self.fd, name, flags, rustix::fs::Mode::empty())?;
        Ok(File::from(fd))
    }

    pub fn open_entry(&self, name: &OsStr) -> io::Result<Opened> {
        use rustix::io::Errno;

        let flags = crate::boundary::descend::MEMBER;
        match rustix::fs::openat(&self.fd, name, flags, rustix::fs::Mode::empty()) {
            Ok(fd) => Ok(Opened::File(File::from(fd))),
            Err(Errno::NOENT) => Ok(Opened::Absent),
            Err(Errno::LOOP) => Ok(Opened::Link),
            Err(error) => Err(error.into()),
        }
    }

    pub fn descend(&self, name: &OsStr) -> io::Result<Self> {
        use rustix::fs::OFlags;

        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.fd, name, flags, rustix::fs::Mode::empty())?;
        Ok(Self {
            path: self.naming(name),
            fd,
        })
    }

    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        use std::os::unix::ffi::OsStringExt;

        let mut entries = Vec::new();
        for entry in rustix::fs::Dir::read_from(&self.fd)? {
            let entry = entry?;
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            let name = OsString::from_vec(name.to_bytes().to_vec());
            let kind = self.kind_of(entry.file_type(), &name)?;
            entries.push(Entry { name, kind });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    fn kind_of(&self, listed: rustix::fs::FileType, name: &OsStr) -> io::Result<EntryKind> {
        use rustix::fs::{AtFlags, FileType};

        let found = match listed {
            FileType::Unknown => {
                let stat = rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
                FileType::from_raw_mode(stat.st_mode)
            }
            known => known,
        };
        Ok(match found {
            FileType::Directory => EntryKind::Directory,
            FileType::RegularFile => EntryKind::File,
            _ => EntryKind::Other,
        })
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

    pub fn open_member(&self, name: &OsStr) -> io::Result<File> {
        File::open(self.naming(name))
    }

    pub fn open_entry(&self, name: &OsStr) -> io::Result<Opened> {
        let path = self.naming(name);
        match std::fs::symlink_metadata(&path) {
            Ok(found) if found.is_symlink() => Ok(Opened::Link),
            Ok(_) => File::open(path).map(Opened::File),
            Err(absent) if absent.kind() == io::ErrorKind::NotFound => Ok(Opened::Absent),
            Err(error) => Err(error),
        }
    }

    pub fn descend(&self, name: &OsStr) -> io::Result<Self> {
        Self::hold(self.naming(name))
    }

    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&self.path)? {
            let entry = entry?;
            let found = entry.file_type()?;
            let kind = if found.is_dir() {
                EntryKind::Directory
            } else if found.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            entries.push(Entry {
                name: entry.file_name(),
                kind,
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
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
    fn a_directory_swapped_for_a_link_after_it_was_held_lists_and_serves_what_was_held() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(base.join("repo/docs/deeper")).unwrap();
        std::fs::write(base.join("repo/docs/inside.md"), "inside\n").unwrap();
        std::fs::create_dir(base.join("outside")).unwrap();
        std::fs::write(base.join("outside/planted.md"), "outside\n").unwrap();
        let repo = held(&base.join("repo"));
        let docs = repo.descend(OsStr::new("docs")).unwrap();
        std::fs::rename(base.join("repo/docs"), base.join("moved")).unwrap();
        std::os::unix::fs::symlink(base.join("outside"), base.join("repo/docs")).unwrap();
        assert!(base.join("repo/docs/planted.md").is_file());

        let listed = docs.entries().unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut docs.open_member(OsStr::new("inside.md")).unwrap(),
            &mut text,
        )
        .unwrap();
        let through_the_link = repo.descend(OsStr::new("docs"));

        assert_eq!(
            listed,
            [
                Entry {
                    name: OsString::from("deeper"),
                    kind: EntryKind::Directory
                },
                Entry {
                    name: OsString::from("inside.md"),
                    kind: EntryKind::File
                },
            ]
        );
        assert_eq!(docs.path(), base.join("repo/docs"));
        assert_eq!(text, "inside\n");
        assert!(through_the_link.is_err());
        assert_eq!(
            repo.entries().unwrap(),
            [Entry {
                name: OsString::from("docs"),
                kind: EntryKind::Other
            }]
        );
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
