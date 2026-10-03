use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    let canonical = |path: &Path| {
        path.canonicalize().map_err(|source| BudgetError::Io {
            path: path.to_path_buf(),
            source,
        })
    };
    if !canonical(&normalize_path(root))?.starts_with(canonical(boundary)?) {
        return Err(BudgetError::OutsideBoundary {
            path: root.to_path_buf(),
            boundary: boundary.to_path_buf(),
        });
    }
    resolve_closure_within(root, boundary)
}

pub fn resolve_closure_within(root: &Path, boundary: &Path) -> Result<Closure, BudgetError> {
    let boundary = boundary.canonicalize().map_err(|source| BudgetError::Io {
        path: boundary.to_path_buf(),
        source,
    })?;
    let root = normalize_path(root);
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut resolved_members: BTreeSet<PathBuf> = BTreeSet::new();
    let mut files: Vec<ContextFile> = Vec::new();
    let mut outside_boundary: Vec<PathBuf> = Vec::new();
    let mut unread: Vec<UnreadLink> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];

    while let Some(path) = stack.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let text = if path == root {
            let text = read_regular_file(&path, FinalComponent::Follow)
                .map_err(|failure| failure.into_error(&path))?;
            resolved_members.extend(directory_resolved(&path));
            text
        } else {
            let resolved = match path.canonicalize() {
                Ok(resolved) => resolved,
                Err(_) => {
                    let first_inside_boundary = match directory_resolved(&path) {
                        Some(member) => {
                            member.starts_with(&boundary) && resolved_members.insert(member)
                        }
                        None => std::path::absolute(&path)
                            .is_ok_and(|absolute| absolute.starts_with(&boundary)),
                    };
                    if first_inside_boundary {
                        unread.extend(unresolved_link(&path));
                    }
                    continue;
                }
            };
            if !resolved.starts_with(&boundary) {
                if resolved.is_file() {
                    outside_boundary.push(path);
                }
                continue;
            }
            if !directory_resolved(&path).is_some_and(|member| resolved_members.insert(member)) {
                continue;
            }
            match read_regular_file(&resolved, FinalComponent::NoFollow) {
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

fn directory_resolved(path: &Path) -> Option<PathBuf> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Some(parent.canonicalize().ok()?.join(path.file_name()?))
}

#[derive(Clone, Copy)]
enum FinalComponent {
    Follow,
    NoFollow,
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

fn open_without_blocking(path: &Path, last: FinalComponent) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(match last {
            FinalComponent::Follow => libc::O_NONBLOCK | libc::O_NOCTTY,
            FinalComponent::NoFollow => libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW,
        });
    }
    #[cfg(not(unix))]
    let _ = last;
    options.open(path)
}

fn read_regular_file(path: &Path, last: FinalComponent) -> Result<String, ReadFailure> {
    let file = open_without_blocking(path, last).map_err(ReadFailure::Io)?;
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
