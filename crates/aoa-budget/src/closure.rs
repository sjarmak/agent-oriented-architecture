use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::boundary::{open_following_links, too_many_links, Boundary, Reached};
use crate::error::BudgetError;
use crate::normalize_path;
use crate::reference::extract_references;

/// A single resolved context file and its on-disk text.
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: PathBuf,
    pub text: String,
}

/// The transitive closure of context files reachable from a root document.
///
/// Produced by [`resolve_closure`]. The root file is always the first entry;
/// every other entry is reachable through markdown links or `@path` includes,
/// resolved relative to the referencing file.
#[derive(Debug, Clone)]
pub struct Closure {
    pub root: PathBuf,
    pub files: Vec<ContextFile>,
    pub outside_boundary: Vec<PathBuf>,
    pub unread: Vec<UnreadLink>,
    pub absent: BTreeSet<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadLink {
    pub path: PathBuf,
    pub reason: UnreadReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnreadReason {
    BrokenSymlink,
    NotRegularFile,
    Unreadable,
    NotUtf8,
}

impl UnreadReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::BrokenSymlink => "a symlink that does not resolve",
            Self::NotRegularFile => "not a regular file",
            Self::Unreadable => "unreadable",
            Self::NotUtf8 => "not UTF-8 text",
        }
    }
}

impl Closure {
    /// Canonical-ish set of the paths in this closure, for membership checks.
    pub fn paths(&self) -> BTreeSet<PathBuf> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }
}

pub const MAX_CONTEXT_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub fn resolve_closure(root: &Path) -> Result<Closure, BudgetError> {
    let boundary = match root.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    resolve_closure_within(root, boundary)
}

pub fn resolve_contained_closure(root: &Path, boundary: &Path) -> Result<Closure, BudgetError> {
    let opened = open_boundary(boundary)?;
    let normalized = normalize_path(root);
    let reached = opened
        .reach(&normalized)
        .map_err(|source| BudgetError::Io {
            path: normalized.clone(),
            source,
        })?;
    match reached {
        Reached::Member { resolved, .. } => {
            resolve(normalized, &opened, RootRead::Beneath(resolved))
        }
        Reached::Outside => Err(BudgetError::OutsideBoundary {
            path: root.to_path_buf(),
            boundary: boundary.to_path_buf(),
        }),
        Reached::Absent { .. } => Err(BudgetError::Io {
            path: normalized,
            source: std::io::ErrorKind::NotFound.into(),
        }),
        Reached::Looping { .. } => Err(BudgetError::Io {
            path: normalized,
            source: too_many_links(),
        }),
    }
}

pub fn resolve_closure_within(root: &Path, boundary: &Path) -> Result<Closure, BudgetError> {
    let root = normalize_path(root);
    let boundary = open_boundary(boundary)?.naming_the_directory_of(&root);
    resolve(root, &boundary, RootRead::FollowingLinks)
}

pub fn leaves_boundary(path: &Path, boundary: &Path) -> Result<bool, BudgetError> {
    let path = normalize_path(path);
    let reached = open_boundary(boundary)?
        .reach(&path)
        .map_err(|source| BudgetError::Io { path, source })?;
    Ok(matches!(reached, Reached::Outside))
}

fn open_boundary(boundary: &Path) -> Result<Boundary, BudgetError> {
    Boundary::open(boundary).map_err(|source| BudgetError::Io {
        path: boundary.to_path_buf(),
        source,
    })
}

enum RootRead {
    FollowingLinks,
    Beneath(PathBuf),
}

fn resolve(
    root: PathBuf,
    boundary: &Boundary,
    root_read: RootRead,
) -> Result<Closure, BudgetError> {
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut resolved_members: BTreeSet<PathBuf> = BTreeSet::new();
    let mut files: Vec<ContextFile> = Vec::new();
    let mut outside_boundary: Vec<PathBuf> = Vec::new();
    let mut unread: Vec<UnreadLink> = Vec::new();
    let mut absent: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];

    while let Some(path) = stack.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let text = if path == root {
            let opened = match &root_read {
                RootRead::FollowingLinks => open_following_links(&path),
                RootRead::Beneath(resolved) => boundary.open_member(resolved),
            };
            let text = read_regular_file(opened).map_err(|failure| failure.into_error(&path))?;
            let reached = boundary.reach(&path).map_err(|source| BudgetError::Io {
                path: path.clone(),
                source,
            })?;
            if let Reached::Member { entry, .. } = reached {
                resolved_members.insert(entry);
            }
            text
        } else {
            let resolved = match boundary.reach(&path) {
                Err(_) => {
                    unread.push(UnreadLink {
                        path,
                        reason: UnreadReason::Unreadable,
                    });
                    continue;
                }
                Ok(Reached::Outside) => {
                    outside_boundary.push(path);
                    continue;
                }
                Ok(Reached::Absent { entry } | Reached::Looping { entry }) => {
                    if resolved_members.insert(entry) {
                        unread.extend(unresolved_link(&path));
                    }
                    absent.insert(path);
                    continue;
                }
                Ok(Reached::Member { entry, resolved }) => {
                    if !resolved_members.insert(entry) {
                        continue;
                    }
                    resolved
                }
            };
            match read_regular_file(boundary.open_member(&resolved)) {
                Ok(text) => text,
                Err(ReadFailure::Oversized) => return Err(ReadFailure::Oversized.into_error(&path)),
                Err(failure) => {
                    unread.extend(failure.unread_reason().map(|reason| UnreadLink {
                        path: path.clone(),
                        reason,
                    }));
                    continue;
                }
            }
        };
        let base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut children: Vec<PathBuf> = extract_references(&text, &base_dir)
            .into_iter()
            .map(|r| normalize_path(&r.target))
            .filter(|p| !visited.contains(p))
            .collect();
        children.reverse();
        stack.extend(children);
        files.push(ContextFile { path, text });
    }

    Ok(Closure {
        root,
        files,
        outside_boundary,
        unread,
        absent,
    })
}

fn unresolved_link(path: &Path) -> Option<UnreadLink> {
    let reason = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => UnreadReason::BrokenSymlink,
        Ok(_) => UnreadReason::Unreadable,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return None
        }
        Err(_) => UnreadReason::Unreadable,
    };
    Some(UnreadLink {
        path: path.to_path_buf(),
        reason,
    })
}

enum ReadFailure {
    Io(std::io::Error),
    Directory,
    NotRegularFile,
    NotUtf8(std::string::FromUtf8Error),
    Oversized,
}

impl ReadFailure {
    fn unread_reason(&self) -> Option<UnreadReason> {
        match self {
            Self::Directory | Self::Oversized => None,
            Self::Io(_) => Some(UnreadReason::Unreadable),
            Self::NotRegularFile => Some(UnreadReason::NotRegularFile),
            Self::NotUtf8(_) => Some(UnreadReason::NotUtf8),
        }
    }

    fn into_error(self, path: &Path) -> BudgetError {
        let path = path.to_path_buf();
        let source = match self {
            Self::Oversized => {
                return BudgetError::Oversized {
                    path,
                    max_bytes: MAX_CONTEXT_FILE_BYTES,
                }
            }
            Self::Io(source) => source,
            Self::Directory | Self::NotRegularFile => {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file")
            }
            Self::NotUtf8(source) => std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        };
        BudgetError::Io { path, source }
    }
}

#[cfg(unix)]
fn refused_as_special_file(source: &std::io::Error) -> bool {
    source.raw_os_error() == Some(rustix::io::Errno::NXIO.raw_os_error())
}

#[cfg(not(unix))]
fn refused_as_special_file(_source: &std::io::Error) -> bool {
    false
}

fn read_regular_file(opened: std::io::Result<std::fs::File>) -> Result<String, ReadFailure> {
    let file = opened.map_err(|source| match refused_as_special_file(&source) {
        true => ReadFailure::NotRegularFile,
        false => ReadFailure::Io(source),
    })?;
    let kind = file.metadata().map_err(ReadFailure::Io)?.file_type();
    if kind.is_dir() {
        return Err(ReadFailure::Directory);
    }
    if !kind.is_file() {
        return Err(ReadFailure::NotRegularFile);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONTEXT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(ReadFailure::Io)?;
    if bytes.len() as u64 > MAX_CONTEXT_FILE_BYTES {
        return Err(ReadFailure::Oversized);
    }
    String::from_utf8(bytes).map_err(ReadFailure::NotUtf8)
}
