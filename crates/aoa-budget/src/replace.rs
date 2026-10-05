use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::path::Path;

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

pub(crate) fn replace(path: &Path, body: &str, standing: &Standing) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut fresh = tempfile::Builder::new()
        .prefix(".aoa-budget-")
        .tempfile_in(dir)?;
    fresh.write_all(body.as_bytes())?;
    standing.dress(fresh.as_file())?;
    fresh.as_file().sync_all()?;
    fresh.persist(path).map_err(|refused| refused.error)?;
    sync_directory(dir)
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
