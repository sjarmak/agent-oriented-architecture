use std::path::{Path, PathBuf};

use aoa_budget::normalize_path;
use ignore::WalkBuilder;

use crate::error::LintError;

const CONTEXT_ROOT_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const IGNORE_FILE_NAMES: [&str; 2] = [".gitignore", ".ignore"];

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
        if let Some(unapplied) = entry.error() {
            return Err(LintError::Walk {
                dir: entry.path().to_path_buf(),
                source: std::io::Error::other(unapplied.to_string()).into(),
            });
        }
        if entry.file_type().is_some_and(|kind| kind.is_dir()) {
            require_readable_ignore_files(entry.path())?;
        }
        let is_file = entry.file_type().is_some_and(|kind| kind.is_file());
        if is_file && is_context_root_name(entry.path()) {
            roots.push(normalize_path(entry.path()));
        }
    }
    Ok(roots)
}

fn require_readable_ignore_files(dir: &Path) -> Result<(), LintError> {
    for name in IGNORE_FILE_NAMES {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(LintError::Walk {
                    dir: path,
                    source: source.into(),
                })
            }
        }
    }
    Ok(())
}

fn is_context_root_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| CONTEXT_ROOT_NAMES.contains(&name))
}
