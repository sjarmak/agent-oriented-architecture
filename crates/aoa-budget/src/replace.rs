use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tempfile::{NamedTempFile, PersistError};

use crate::error::{BudgetError, LeftBehindTemp};

pub(crate) struct Standing {
    permissions: Permissions,
    #[cfg(unix)]
    owner: Option<(u32, u32)>,
    #[cfg(target_os = "linux")]
    attributes: Vec<(std::ffi::CString, Vec<u8>)>,
}

impl Standing {
    pub(crate) fn of_writable(path: &Path) -> io::Result<Option<Self>> {
        match std::fs::symlink_metadata(path) {
            Ok(found) if found.is_file() => {}
            Ok(_) => return Ok(None),
            Err(absent) if absent.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let file = open_for_writing(path)?;
        let found = file.metadata()?;
        if !found.is_file() {
            return Ok(None);
        }
        Ok(Some(Self {
            permissions: found.permissions(),
            #[cfg(unix)]
            owner: Some(owner(&found)),
            #[cfg(target_os = "linux")]
            attributes: carried_attributes(&file)?,
        }))
    }

    pub(crate) fn of_a_new_file(&self) -> Self {
        Self {
            permissions: self.permissions.clone(),
            #[cfg(unix)]
            owner: None,
            #[cfg(target_os = "linux")]
            attributes: Vec::new(),
        }
    }

    fn dress(&self, fresh: &File) -> io::Result<()> {
        #[cfg(unix)]
        if let Some((user, group)) = self.owner {
            if owner(&fresh.metadata()?) != (user, group) {
                std::os::unix::fs::fchown(fresh, Some(user), Some(group))?;
            }
        }
        #[cfg(target_os = "linux")]
        for (name, value) in &self.attributes {
            rustix::fs::fsetxattr(
                fresh,
                name.as_c_str(),
                value,
                rustix::fs::XattrFlags::empty(),
            )?;
        }
        fresh.set_permissions(self.permissions.clone())
    }
}

#[derive(Debug)]
pub(crate) enum Unplaced {
    Refused(io::Error),
    LeftBehind {
        source: io::Error,
        left: Vec<LeftBehindTemp>,
    },
}

impl Unplaced {
    pub(crate) fn at(self, path: &Path) -> BudgetError {
        let path = path.to_path_buf();
        match self {
            Unplaced::Refused(source) => BudgetError::Io { path, source },
            Unplaced::LeftBehind { source, left } => {
                BudgetError::TempFileLeftBehind { path, left, source }
            }
        }
    }

    pub(crate) fn withdrawing(self, prepared: Replacement) -> Self {
        let (source, mut left) = match self {
            Unplaced::Refused(source) => (source, Vec::new()),
            Unplaced::LeftBehind { source, left } => (source, left),
        };
        left.extend(withdraw(prepared.fresh));
        Self::leaving(source, left)
    }

    fn leaving(source: io::Error, left: Vec<LeftBehindTemp>) -> Self {
        if left.is_empty() {
            Unplaced::Refused(source)
        } else {
            Unplaced::LeftBehind { source, left }
        }
    }
}

fn withdrawn(fresh: NamedTempFile, source: io::Error) -> Unplaced {
    Unplaced::leaving(source, withdraw(fresh).into_iter().collect())
}

fn withdraw(fresh: NamedTempFile) -> Option<LeftBehindTemp> {
    let temp = fresh.path().to_path_buf();
    fresh
        .close()
        .err()
        .map(|removal| LeftBehindTemp { temp, removal })
}

pub(crate) struct Placing<'a> {
    pub(crate) rename: &'a dyn Fn(NamedTempFile, &Path) -> Result<(), PersistError>,
    pub(crate) sync_directory: &'a dyn Fn(&Path) -> io::Result<()>,
}

impl Placing<'static> {
    pub(crate) const REAL: Self = Self {
        rename: &persist,
        sync_directory: &sync_directory,
    };
}

fn persist(fresh: NamedTempFile, to: &Path) -> Result<(), PersistError> {
    fresh.persist(to).map(drop)
}

pub(crate) struct Replacement {
    fresh: NamedTempFile,
    path: PathBuf,
}

impl Replacement {
    pub(crate) fn prepare(path: &Path, body: &str, standing: &Standing) -> Result<Self, Unplaced> {
        let mut fresh = tempfile::Builder::new()
            .prefix(".aoa-budget-")
            .tempfile_in(directory_of(path))
            .map_err(Unplaced::Refused)?;
        let filled = fresh
            .write_all(body.as_bytes())
            .and_then(|()| standing.dress(fresh.as_file()))
            .and_then(|()| fresh.as_file().sync_all());
        match filled {
            Ok(()) => Ok(Self {
                fresh,
                path: path.to_path_buf(),
            }),
            Err(source) => Err(withdrawn(fresh, source)),
        }
    }

    pub(crate) fn put(self, placing: &Placing) -> Result<(), Unplaced> {
        match (placing.rename)(self.fresh, &self.path) {
            Ok(()) => (placing.sync_directory)(directory_of(&self.path)).map_err(Unplaced::Refused),
            Err(refused) => Err(withdrawn(refused.file, refused.error)),
        }
    }
}

fn directory_of(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

#[cfg(unix)]
fn open_for_writing(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(flags.bits() as i32)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_writing(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

#[cfg(unix)]
fn owner(found: &std::fs::Metadata) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;

    (found.uid(), found.gid())
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
const CARRIED_ATTRIBUTE_PREFIXES: [&[u8]; 2] = [b"user.", b"system.posix_acl_"];

#[cfg(target_os = "linux")]
fn carried_attributes(file: &File) -> io::Result<Vec<(std::ffi::CString, Vec<u8>)>> {
    let names = match sized(|buffer| rustix::fs::flistxattr(file, buffer)) {
        Ok(names) => names,
        Err(rustix::io::Errno::NOTSUP) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    names
        .split(|byte| *byte == 0)
        .filter(|name| {
            CARRIED_ATTRIBUTE_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .map(|name| {
            let name = std::ffi::CString::new(name)?;
            let value = sized(|buffer| rustix::fs::fgetxattr(file, name.as_c_str(), buffer))?;
            Ok((name, value))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn sized(read: impl Fn(&mut [u8]) -> rustix::io::Result<usize>) -> rustix::io::Result<Vec<u8>> {
    let mut buffer = vec![0; read(&mut [])?];
    let filled = read(&mut buffer)?;
    buffer.truncate(filled);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temporary_file_that_can_be_neither_placed_nor_removed_is_named_in_the_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        std::fs::write(&path, "standing").unwrap();
        let standing = Standing::of_writable(&path).unwrap().unwrap();
        let replacement = Replacement::prepare(&path, "fresh", &standing).unwrap();
        let temp = replacement.fresh.path().to_path_buf();
        std::fs::remove_file(&temp).unwrap();
        std::fs::create_dir(&temp).unwrap();
        std::fs::write(temp.join("held"), "").unwrap();

        let refused = replacement.put(&Placing::REAL);

        assert!(
            matches!(
                &refused,
                Err(Unplaced::LeftBehind { left, .. })
                    if matches!(left.as_slice(), [named] if named.temp == temp)
            ),
            "{refused:?}"
        );
        let reported = refused.unwrap_err().at(&path).to_string();
        assert!(reported.contains(&temp.display().to_string()), "{reported}");
        assert!(temp.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "standing");
    }

    #[test]
    fn withdrawing_a_second_unremovable_file_names_both_in_the_order_they_were_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        std::fs::write(&path, "standing").unwrap();
        let standing = Standing::of_writable(&path).unwrap().unwrap();
        let mut temps = Vec::new();
        let mut stranded = || {
            let replacement = Replacement::prepare(&path, "fresh", &standing).unwrap();
            let temp = replacement.fresh.path().to_path_buf();
            std::fs::remove_file(&temp).unwrap();
            std::fs::create_dir(&temp).unwrap();
            std::fs::write(temp.join("held"), "").unwrap();
            temps.push(temp);
            replacement
        };
        let first = stranded();
        let second = stranded();

        let refused = first.put(&Placing::REAL).unwrap_err().withdrawing(second);

        let Unplaced::LeftBehind { left, .. } = &refused else {
            panic!("{refused:?}");
        };
        let named: Vec<_> = left.iter().map(|file| file.temp.clone()).collect();
        assert_eq!(named, temps);
        assert!(temps.iter().all(|temp| temp.join("held").exists()));
    }

    #[test]
    fn a_temporary_file_that_was_removed_leaves_the_refusal_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        std::fs::write(&path, "standing").unwrap();
        let standing = Standing::of_writable(&path).unwrap().unwrap();
        let replacement = Replacement::prepare(&path, "fresh", &standing).unwrap();
        let refusing = Placing {
            rename: &|fresh, _to| {
                Err(PersistError {
                    error: io::Error::other("refused by the test"),
                    file: fresh,
                })
            },
            sync_directory: Placing::REAL.sync_directory,
        };

        let refused = replacement.put(&refusing);

        assert!(matches!(&refused, Err(Unplaced::Refused(_))), "{refused:?}");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["big.md"]);
    }
}
