use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aoa_budget::{leaves_boundary, normalize_path};
use ignore::{DirEntry, WalkBuilder};

use crate::error::LintError;

const CONTEXT_ROOT_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const IGNORE_FILE_NAMES: [&str; 2] = [".gitignore", ".ignore"];

pub fn discover_context_roots(dir: &Path) -> Result<Vec<PathBuf>, LintError> {
    refuse_ignore_file_links_leaving(dir, dir)?;
    let refused = Arc::new(Mutex::new(None));
    let mut walker = WalkBuilder::new(dir)
        .hidden(false)
        .require_git(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(entered(dir, &refused))
        .sort_by_file_path(Path::cmp)
        .build();

    let mut roots = Vec::new();
    loop {
        let entry = walker.next();
        if let Some(refused) = held(&refused).take() {
            return Err(refused);
        }
        let Some(entry) = entry else {
            return Ok(roots);
        };
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
}

type Refused = Arc<Mutex<Option<LintError>>>;

fn held(refused: &Refused) -> MutexGuard<'_, Option<LintError>> {
    refused.lock().unwrap_or_else(PoisonError::into_inner)
}

fn entered(linted: &Path, refused: &Refused) -> impl Fn(&DirEntry) -> bool + Send + Sync + 'static {
    let linted = linted.to_path_buf();
    let refused = Arc::clone(refused);
    move |entry| {
        if entry.file_name() == ".git" {
            return false;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
            return true;
        }
        match refuse_ignore_file_links_leaving(&linted, entry.path()) {
            Ok(()) => true,
            Err(error) => {
                held(&refused).get_or_insert(error);
                false
            }
        }
    }
}

fn is_walked_dir(entry: &DirEntry) -> bool {
    entry.file_type().is_some_and(|kind| kind.is_dir())
        || (entry.depth() == 0 && entry.path().is_dir())
}

fn refuse_ignore_file_links_leaving(linted: &Path, dir: &Path) -> Result<(), LintError> {
    for name in IGNORE_FILE_NAMES {
        let path = dir.join(name);
        let is_link = path
            .symlink_metadata()
            .is_ok_and(|found| found.file_type().is_symlink());
        if is_link && leaves_boundary(&path, linted)? {
            return Err(LintError::IgnoreFileOutside {
                path,
                dir: linted.to_path_buf(),
            });
        }
    }
    Ok(())
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
