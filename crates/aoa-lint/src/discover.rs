use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use aoa_budget::normalize_path;
use ignore::{DirEntry, WalkBuilder};

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
        if is_walked_dir(&entry) {
            require_readable_ignore_files(entry.path())?;
        }
        let is_file = entry.file_type().is_some_and(|kind| kind.is_file());
        if is_file && is_context_root_name(entry.path()) {
            roots.push(normalize_path(entry.path()));
        }
    }
    Ok(roots)
}

fn is_walked_dir(entry: &DirEntry) -> bool {
    entry.file_type().is_some_and(|kind| kind.is_dir())
        || (entry.depth() == 0 && entry.path().is_dir())
}

fn require_readable_ignore_files(dir: &Path) -> Result<(), LintError> {
    for name in IGNORE_FILE_NAMES {
        let path = dir.join(name);
        if let Err(source) = read_through(&path) {
            return Err(LintError::Walk {
                dir: path,
                source: source.into(),
            });
        }
    }
    Ok(())
}

fn read_through(path: &Path) -> std::io::Result<()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(source),
    };
    for line in BufReader::new(file).lines() {
        line?;
    }
    Ok(())
}

fn is_context_root_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| CONTEXT_ROOT_NAMES.contains(&name))
}
