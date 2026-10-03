use std::path::{Path, PathBuf};

use aoa_budget::normalize_path;
use ignore::WalkBuilder;

use crate::error::LintError;

const CONTEXT_ROOT_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

pub fn discover_context_roots(dir: &Path) -> Result<Vec<PathBuf>, LintError> {
    let walker = WalkBuilder::new(dir)
        .hidden(false)
        .require_git(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .sort_by_file_path(Path::cmp)
        .build();

    let mut roots = Vec::new();
    for entry in walker {
        let entry = entry.map_err(|source| LintError::Walk {
            dir: dir.to_path_buf(),
            source,
        })?;
        let is_file = entry.file_type().is_some_and(|kind| kind.is_file());
        if is_file && is_context_root_name(entry.path()) {
            roots.push(normalize_path(entry.path()));
        }
    }
    Ok(roots)
}

fn is_context_root_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| CONTEXT_ROOT_NAMES.contains(&name))
}
