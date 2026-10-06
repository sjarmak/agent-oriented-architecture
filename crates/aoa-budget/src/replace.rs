use std::ffi::{OsStr, OsString};
use std::fs::{File, Permissions};
use std::io::{self, Write};
use std::path::Path;

use crate::directory::{Directory, Placement};
use crate::error::{BudgetError, LeftBehindTemp};

const FRESH_PREFIX: &str = ".aoa-budget-";

pub(crate) struct Standing {
    permissions: Permissions,
    #[cfg(unix)]
    owner: Option<(u32, u32)>,
    #[cfg(target_os = "linux")]
    attributes: Vec<(std::ffi::CString, Vec<u8>)>,
}

impl Standing {
    pub(crate) fn of_writable(directory: &Directory, name: &OsStr) -> io::Result<Option<Self>> {
        let file = match directory.open_writable(name) {
            Ok(file) => file,
            Err(absent) if absent.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
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

    pub(crate) fn withdrawing(self, prepared: Replacement<'_>) -> Self {
        let (source, mut left) = match self {
            Unplaced::Refused(source) => (source, Vec::new()),
            Unplaced::LeftBehind { source, left } => (source, left),
        };
        left.extend(prepared.withdraw());
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

pub(crate) struct Placing<'a> {
    pub(crate) rename: &'a dyn Fn(&Directory, &OsStr, &OsStr, Placement) -> io::Result<()>,
    pub(crate) sync_directory: &'a dyn Fn(&Directory) -> io::Result<()>,
}

impl Placing<'static> {
    pub(crate) const REAL: Self = Self {
        rename: &Directory::place,
        sync_directory: &Directory::sync,
    };
}

pub(crate) struct Replacement<'d> {
    directory: &'d Directory,
    fresh: OsString,
    target: OsString,
    placement: Placement,
}

impl<'d> Replacement<'d> {
    pub(crate) fn prepare(
        directory: &'d Directory,
        target: &OsStr,
        placement: Placement,
        body: &str,
        standing: &Standing,
    ) -> Result<Self, Unplaced> {
        let (fresh, mut file) = directory
            .create_fresh(FRESH_PREFIX)
            .map_err(Unplaced::Refused)?;
        let filled = file
            .write_all(body.as_bytes())
            .and_then(|()| standing.dress(&file))
            .and_then(|()| file.sync_all());
        drop(file);
        let prepared = Self {
            directory,
            fresh,
            target: target.to_os_string(),
            placement,
        };
        match filled {
            Ok(()) => Ok(prepared),
            Err(source) => Err(prepared.withdrawn(source)),
        }
    }

    pub(crate) fn put(self, placing: &Placing) -> Result<(), Unplaced> {
        match (placing.rename)(self.directory, &self.fresh, &self.target, self.placement) {
            Ok(()) => (placing.sync_directory)(self.directory).map_err(Unplaced::Refused),
            Err(refused) => Err(self.withdrawn(refused)),
        }
    }

    fn withdrawn(self, source: io::Error) -> Unplaced {
        Unplaced::leaving(source, self.withdraw().into_iter().collect())
    }

    fn withdraw(self) -> Option<LeftBehindTemp> {
        let temp = self.directory.naming(&self.fresh);
        self.directory
            .remove(&self.fresh)
            .err()
            .map(|removal| LeftBehindTemp { temp, removal })
    }
}

#[cfg(unix)]
fn owner(found: &std::fs::Metadata) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;

    (found.uid(), found.gid())
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
    use std::path::PathBuf;

    use super::*;
    use crate::boundary::Boundary;

    fn held(dir: &Path) -> (Directory, PathBuf) {
        let path = dir.join("big.md");
        std::fs::write(&path, "standing").unwrap();
        let boundary = Boundary::open(dir).unwrap();
        let directory = boundary
            .hold_directory(&dir.canonicalize().unwrap())
            .unwrap();
        (directory, path)
    }

    fn strand(temp: &Path) {
        std::fs::remove_file(temp).unwrap();
        std::fs::create_dir(temp).unwrap();
        std::fs::write(temp.join("held"), "").unwrap();
    }

    fn prepared<'d>(directory: &'d Directory, standing: &Standing) -> Replacement<'d> {
        Replacement::prepare(
            directory,
            OsStr::new("big.md"),
            Placement::Replacing,
            "fresh",
            standing,
        )
        .unwrap()
    }

    #[test]
    fn a_temporary_file_that_can_be_neither_placed_nor_removed_is_named_in_the_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (directory, path) = held(dir.path());
        let standing = Standing::of_writable(&directory, OsStr::new("big.md"))
            .unwrap()
            .unwrap();
        let replacement = prepared(&directory, &standing);
        let temp = directory.naming(&replacement.fresh);
        strand(&temp);

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
        let (directory, _path) = held(dir.path());
        let standing = Standing::of_writable(&directory, OsStr::new("big.md"))
            .unwrap()
            .unwrap();
        let mut temps = Vec::new();
        let mut stranded = || {
            let replacement = prepared(&directory, &standing);
            let temp = directory.naming(&replacement.fresh);
            strand(&temp);
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
        let (directory, _path) = held(dir.path());
        let standing = Standing::of_writable(&directory, OsStr::new("big.md"))
            .unwrap()
            .unwrap();
        let replacement = prepared(&directory, &standing);
        let refusing = Placing {
            rename: &|_directory, _from, _to, _placement| {
                Err(io::Error::other("refused by the test"))
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

    #[test]
    fn a_replacement_that_creates_does_not_replace_an_entry_made_at_its_name_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (directory, _path) = held(dir.path());
        let standing = Standing::of_writable(&directory, OsStr::new("big.md"))
            .unwrap()
            .unwrap();
        let archive = dir.path().join("big.archive.md");
        let replacement = Replacement::prepare(
            &directory,
            OsStr::new("big.archive.md"),
            Placement::Creating,
            "fresh",
            &standing,
        )
        .unwrap();
        std::fs::write(&archive, "planted").unwrap();

        let refused = replacement.put(&Placing::REAL);

        assert!(
            matches!(
                &refused,
                Err(Unplaced::Refused(source)) if source.kind() == io::ErrorKind::AlreadyExists
            ),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_to_string(&archive).unwrap(), "planted");
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["big.archive.md", "big.md"]);
    }
}
