use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::BudgetError;
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
}

impl Closure {
    /// Canonical-ish set of the paths in this closure, for membership checks.
    pub fn paths(&self) -> BTreeSet<PathBuf> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }
}

/// Resolve the transitive closure of context files starting at `root`.
///
/// Performs a cycle-safe depth-first walk: each file is read once, its
/// references extracted and queued, and already-visited paths are skipped so a
/// reference cycle terminates. A reference that does not resolve to a readable
/// file is skipped (it may be a relative doc link outside the context tree);
/// failure to read the explicitly requested `root`, however, is an error.
pub fn resolve_closure(root: &Path) -> Result<Closure, BudgetError> {
    let root = normalize(root);
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut files: Vec<ContextFile> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];

    while let Some(path) = stack.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(source) => {
                if path == root {
                    return Err(BudgetError::Io { path, source });
                }
                continue;
            }
        };
        let base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut children: Vec<PathBuf> = extract_references(&text, &base_dir)
            .into_iter()
            .map(|r| normalize(&r.target))
            .filter(|p| !visited.contains(p))
            .collect();
        // Reverse so the stack pops children in source order (stable output).
        children.reverse();
        stack.extend(children);
        files.push(ContextFile { path, text });
    }

    Ok(Closure { root, files })
}

/// Lexically normalize a path (resolve `.` and `..` components) without
/// touching the filesystem, so equivalent references compare equal.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        use std::path::Component::*;
        match component {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
