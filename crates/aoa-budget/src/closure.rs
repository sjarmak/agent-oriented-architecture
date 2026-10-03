use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

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
    let mut stack: Vec<PathBuf> = vec![root.clone()];

    while let Some(path) = stack.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let text = if path == root {
            let text = read_regular_file(&path, &path)?;
            resolved_members.extend(directory_resolved(&path));
            text
        } else {
            let Ok(resolved) = path.canonicalize() else {
                continue;
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
            match read_regular_file(&resolved, &path) {
                Ok(text) => text,
                Err(oversized @ BudgetError::Oversized { .. }) => return Err(oversized),
                Err(_) => continue,
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
    })
}

fn directory_resolved(path: &Path) -> Option<PathBuf> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Some(parent.canonicalize().ok()?.join(path.file_name()?))
}

fn read_regular_file(path: &Path, reported: &Path) -> Result<String, BudgetError> {
    let io_err = |source| BudgetError::Io {
        path: reported.to_path_buf(),
        source,
    };
    let meta = std::fs::metadata(path).map_err(io_err)?;
    if !meta.is_file() {
        return Err(io_err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        )));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io_err)?
        .take(MAX_CONTEXT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err)?;
    if bytes.len() as u64 > MAX_CONTEXT_FILE_BYTES {
        return Err(BudgetError::Oversized {
            path: reported.to_path_buf(),
            max_bytes: MAX_CONTEXT_FILE_BYTES,
        });
    }
    String::from_utf8(bytes)
        .map_err(|source| io_err(std::io::Error::new(std::io::ErrorKind::InvalidData, source)))
}
