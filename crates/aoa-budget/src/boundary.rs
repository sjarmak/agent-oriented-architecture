use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::directory::Directory;
use crate::normalize_path;

const MAX_LINKS_FOLLOWED: usize = 256;

pub(crate) struct Boundary {
    path: PathBuf,
    names: Vec<(PathBuf, PathBuf)>,
    #[cfg(unix)]
    directory: std::os::fd::OwnedFd,
}

pub(crate) enum Reached {
    Outside,
    Absent { entry: PathBuf },
    Looping { entry: PathBuf },
    Member { entry: PathBuf, resolved: PathBuf },
}

enum Lost {
    Outside,
    Absent,
    Looping,
    Failed(io::Error),
}

impl From<io::Error> for Lost {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Self::Absent,
            _ => Self::Failed(error),
        }
    }
}

impl Boundary {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let opened = Self::at(path.canonicalize()?)?;
        let directory = opened.path.clone();
        Ok(opened.named(&std::path::absolute(path)?, directory))
    }

    fn at(path: PathBuf) -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            directory: open_directory(&path)?,
            names: vec![(path.clone(), path.clone())],
            path,
        })
    }

    fn named(mut self, name: &Path, directory: PathBuf) -> Self {
        self.names.push((normalize_path(name), directory));
        self
    }

    pub(crate) fn naming_the_directory_of(self, file: &Path) -> Self {
        let directory = match file.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let Ok(name) = std::path::absolute(directory) else {
            return self;
        };
        if self.locate(&normalize_path(&name)).is_some() {
            return self;
        }
        match directory.canonicalize() {
            Ok(resolved) if resolved.starts_with(&self.path) => self.named(&name, resolved),
            _ => self,
        }
    }

    fn locate<'a>(&'a self, absolute: &'a Path) -> Option<(&'a Path, &'a Path)> {
        self.names.iter().find_map(|(name, directory)| {
            Some((directory.as_path(), absolute.strip_prefix(name).ok()?))
        })
    }

    pub(crate) fn reach(&self, target: &Path) -> io::Result<Reached> {
        let mut followed = 0;
        let Ok(absolute) = std::path::absolute(target) else {
            return Ok(Reached::Outside);
        };
        let absolute = normalize_path(&absolute);
        let Some((directory, beneath)) = self.locate(&absolute) else {
            return Ok(Reached::Outside);
        };
        let mut resolved = directory.to_path_buf();
        let mut entry = resolved.clone();
        let mut names = beneath.components();
        while let Some(name) = names.next() {
            entry = resolved.join(name);
            let lost = match self.step(resolved, name, &mut followed) {
                Ok(next) => {
                    resolved = next;
                    continue;
                }
                Err(lost) => lost,
            };
            let entry = entry.join(names.as_path());
            return match lost {
                Lost::Outside => Ok(Reached::Outside),
                Lost::Absent => Ok(Reached::Absent { entry }),
                Lost::Looping => Ok(Reached::Looping { entry }),
                Lost::Failed(error) => Err(error),
            };
        }
        Ok(Reached::Member { entry, resolved })
    }

    fn step(
        &self,
        mut at: PathBuf,
        name: Component<'_>,
        followed: &mut usize,
    ) -> Result<PathBuf, Lost> {
        let mut at_a_directory = true;
        let mut pending: Vec<OsString> = vec![name.as_os_str().to_os_string()];
        while let Some(name) = pending.pop() {
            if !at_a_directory {
                return Err(Lost::Absent);
            }
            if name == "." {
                continue;
            }
            if name == ".." {
                if at == self.path {
                    return Err(Lost::Outside);
                }
                std::fs::symlink_metadata(at.join(".."))?;
                at.pop();
                continue;
            }
            let next = at.join(&name);
            let kind = std::fs::symlink_metadata(&next)?.file_type();
            if !kind.is_symlink() {
                at_a_directory = kind.is_dir();
                at = next;
                continue;
            }
            if *followed == MAX_LINKS_FOLLOWED {
                return Err(Lost::Looping);
            }
            let target = std::fs::read_link(&next)?;
            *followed += 1;
            pending.extend(names_a_directory(&target).then(|| OsString::from(".")));
            let target = match target.has_root() {
                false => target.as_path(),
                true => {
                    let (directory, beneath) = self.locate(&target).ok_or(Lost::Outside)?;
                    at = directory.to_path_buf();
                    beneath
                }
            };
            pending.extend(target.components().rev().filter_map(|part| match part {
                Component::Normal(_) | Component::ParentDir => {
                    Some(part.as_os_str().to_os_string())
                }
                Component::CurDir | Component::RootDir | Component::Prefix(_) => None,
            }));
        }
        Ok(at)
    }

    fn beneath<'a>(&self, resolved: &'a Path) -> io::Result<&'a Path> {
        resolved.strip_prefix(&self.path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "member is not beneath the boundary",
            )
        })
    }

    pub(crate) fn split(&self, resolved: &Path) -> io::Result<(PathBuf, OsString)> {
        self.beneath(resolved)?;
        match (resolved.parent(), resolved.file_name()) {
            (Some(parent), Some(name)) => Ok((parent.to_path_buf(), name.to_os_string())),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "member path names no file",
            )),
        }
    }

    #[cfg(unix)]
    fn descend(&self, resolved: &Path) -> io::Result<std::os::fd::OwnedFd> {
        let mut current = self.directory.try_clone()?;
        for component in self.beneath(resolved)?.components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "member path is not canonical",
                ));
            };
            let flags = descend::DIRECTORY | rustix::fs::OFlags::NOFOLLOW;
            current = rustix::fs::openat(&current, name, flags, rustix::fs::Mode::empty())?;
        }
        Ok(current)
    }

    #[cfg(unix)]
    pub(crate) fn hold_directory(&self, resolved: &Path) -> io::Result<Directory> {
        Directory::hold(&self.descend(resolved)?, resolved.to_path_buf())
    }

    #[cfg(not(unix))]
    pub(crate) fn hold_directory(&self, resolved: &Path) -> io::Result<Directory> {
        self.beneath(resolved)?;
        Directory::hold(resolved.to_path_buf())
    }

    #[cfg(unix)]
    pub(crate) fn hold_root(&self, named: PathBuf) -> io::Result<Directory> {
        Directory::hold(&self.directory, named)
    }

    #[cfg(not(unix))]
    pub(crate) fn hold_root(&self, named: PathBuf) -> io::Result<Directory> {
        Directory::hold(named)
    }

    #[cfg(unix)]
    pub(crate) fn open_member(&self, resolved: &Path) -> io::Result<File> {
        let (directory, name) = match self.beneath(resolved)?.as_os_str().is_empty() {
            true => (self.directory.try_clone()?, OsString::from(".")),
            false => {
                let (parent, name) = self.split(resolved)?;
                (self.descend(&parent)?, name)
            }
        };
        let flags = descend::MEMBER;
        let fd = rustix::fs::openat(&directory, name, flags, rustix::fs::Mode::empty())?;
        Ok(File::from(fd))
    }

    #[cfg(not(unix))]
    pub(crate) fn open_member(&self, resolved: &Path) -> io::Result<File> {
        self.split(resolved)?;
        File::open(resolved)
    }
}

#[cfg(unix)]
pub(crate) fn too_many_links() -> io::Error {
    rustix::io::Errno::LOOP.into()
}

#[cfg(not(unix))]
pub(crate) fn too_many_links() -> io::Error {
    io::Error::other("too many levels of symbolic links")
}

fn names_a_directory(target: &Path) -> bool {
    let is_separator = |byte: &u8| std::path::is_separator(char::from(*byte));
    match target.as_os_str().as_encoded_bytes() {
        [.., last] if is_separator(last) => true,
        [.., before, b'.'] => is_separator(before),
        _ => false,
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
pub(crate) mod descend {
    use rustix::fs::OFlags;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    const TRAVERSE: OFlags = OFlags::PATH;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const TRAVERSE: OFlags = OFlags::RDONLY;

    pub(super) const DIRECTORY: OFlags = TRAVERSE.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC);
    pub(crate) const MEMBER: OFlags = OFlags::RDONLY
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
