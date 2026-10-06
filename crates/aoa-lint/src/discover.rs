use std::collections::BTreeMap;
use std::fs::Metadata;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use aoa_budget::{normalize_path, BudgetError, Enclosure};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::LintError;

const CONTEXT_ROOT_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const IGNORE_FILE_NAMES: [&str; 2] = [".gitignore", ".ignore"];
const UTF8_BOM: char = '\u{feff}';

pub fn discover_context_roots(dir: &Path) -> Result<Vec<PathBuf>, LintError> {
    discover_checking(dir, &|_| ())
}

fn discover_checking(dir: &Path, checked: &dyn Fn(&Path)) -> Result<Vec<PathBuf>, LintError> {
    let mut rules = Rules::for_linting(dir, checked)?;
    let mut roots = Vec::new();
    rules.enter(dir)?;
    let mut walking = vec![entries_of(dir)?];
    while let Some(entries) = walking.last_mut() {
        let Some(entry) = entries.next() else {
            walking.pop();
            continue;
        };
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|source| walk_failed(&path, source))?;
        if kind.is_dir() && entry.file_name() != ".git" && rules.admit(&path, true) {
            rules.enter(&path)?;
            walking.push(entries_of(&path)?);
        } else if kind.is_file() && is_context_root_name(&path) && rules.admit(&path, false) {
            roots.push(normalize_path(&path));
        }
    }
    Ok(roots)
}

fn entries_of(directory: &Path) -> Result<std::vec::IntoIter<std::fs::DirEntry>, LintError> {
    let mut entries = std::fs::read_dir(directory)
        .and_then(|read| read.collect::<io::Result<Vec<_>>>())
        .map_err(|source| walk_failed(directory, source))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries.into_iter())
}

fn walk_failed(path: &Path, source: io::Error) -> LintError {
    LintError::Walk {
        dir: path.to_path_buf(),
        source: source.into(),
    }
}

struct Rules<'a> {
    linted: PathBuf,
    enclosure: Enclosure,
    matchers: BTreeMap<PathBuf, Gitignore>,
    checked: &'a dyn Fn(&Path),
}

impl<'a> Rules<'a> {
    fn for_linting(dir: &Path, checked: &'a dyn Fn(&Path)) -> Result<Self, LintError> {
        let enclosure = Enclosure::open(dir).map_err(|refused| match refused {
            BudgetError::Io { path, source } => walk_failed(&path, source),
            other => LintError::Budget(other),
        })?;
        Ok(Self {
            linted: dir.to_path_buf(),
            enclosure,
            matchers: BTreeMap::new(),
            checked,
        })
    }

    fn enter(&mut self, dir: &Path) -> Result<(), LintError> {
        let mut builder = GitignoreBuilder::new(dir);
        for name in IGNORE_FILE_NAMES {
            let path = dir.join(name);
            if let Some(text) = self.read_ignore_file(&path)? {
                add_lines(&mut builder, &path, &text)?;
            }
        }
        let matcher = builder.build().map_err(|source| LintError::Walk {
            dir: dir.to_path_buf(),
            source,
        })?;
        self.matchers.insert(dir.to_path_buf(), matcher);
        Ok(())
    }

    fn admit(&self, path: &Path, is_dir: bool) -> bool {
        for ancestor in path.ancestors().skip(1) {
            if let Some(matcher) = self.matchers.get(ancestor) {
                let found = matcher.matched(path, is_dir);
                if found.is_ignore() {
                    return false;
                }
                if found.is_whitelist() {
                    return true;
                }
            }
            if ancestor == self.linted {
                break;
            }
        }
        true
    }

    fn read_ignore_file(&self, path: &Path) -> Result<Option<String>, LintError> {
        let Some(mut file) = self.open(path)? else {
            return Ok(None);
        };
        let found = file
            .metadata()
            .map_err(|source| walk_failed(path, source))?;
        if found.is_dir() {
            return Err(walk_failed(path, io::ErrorKind::IsADirectory.into()));
        }
        if !found.is_file() {
            return Err(LintError::IgnoreFileNotRegular {
                path: path.to_path_buf(),
            });
        }
        if has_another_name(&found) {
            return Err(LintError::IgnoreFileHardLinked {
                path: path.to_path_buf(),
            });
        }
        (self.checked)(path);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| walk_failed(path, source))?;
        String::from_utf8(bytes).map(Some).map_err(|invalid| {
            walk_failed(
                path,
                io::Error::new(io::ErrorKind::InvalidData, invalid.utf8_error()),
            )
        })
    }

    fn open(&self, path: &Path) -> Result<Option<std::fs::File>, LintError> {
        self.enclosure
            .open_file(path)
            .map_err(|refused| match refused {
                BudgetError::OutsideBoundary { path, .. } => LintError::IgnoreFileOutside {
                    path,
                    dir: self.linted.clone(),
                },
                BudgetError::Io { path, source } => walk_failed(&path, source),
                other => LintError::Budget(other),
            })
    }
}

fn add_lines(builder: &mut GitignoreBuilder, path: &Path, text: &str) -> Result<(), LintError> {
    for (index, line) in text.lines().enumerate() {
        let line = if index == 0 {
            line.trim_start_matches(UTF8_BOM)
        } else {
            line
        };
        if let Err(err) = builder.add_line(Some(path.to_path_buf()), line) {
            let at_line = ignore::Error::WithLineNumber {
                line: index as u64 + 1,
                err: Box::new(err),
            };
            return Err(LintError::Walk {
                dir: path.to_path_buf(),
                source: ignore::Error::WithPath {
                    path: path.to_path_buf(),
                    err: Box::new(at_line),
                },
            });
        }
    }
    Ok(())
}

#[cfg(unix)]
fn has_another_name(file: &Metadata) -> bool {
    std::os::unix::fs::MetadataExt::nlink(file) > 1
}

#[cfg(not(unix))]
fn has_another_name(_file: &Metadata) -> bool {
    false
}

fn is_context_root_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| CONTEXT_ROOT_NAMES.contains(&name))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_directory_swapped_for_a_link_between_the_check_and_the_read_keeps_the_rules_that_were_checked(
    ) {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let repo = base.join("repo");
        let docs = repo.join("docs");
        let other = repo.join("other");
        let elsewhere = base.join("elsewhere");
        for dir in [&docs, &other] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("AGENTS.md"), "").unwrap();
            std::fs::write(dir.join("CLAUDE.md"), "").unwrap();
        }
        std::fs::write(docs.join(".gitignore"), "AGENTS.md\n").unwrap();
        std::fs::write(other.join(".gitignore"), "CLAUDE.md\n").unwrap();
        let swapping = |checked: &Path| {
            if checked == docs.join(".gitignore") {
                std::fs::rename(&docs, &elsewhere).unwrap();
                std::os::unix::fs::symlink(&other, &docs).unwrap();
            }
        };

        let roots = discover_checking(&repo, &swapping).unwrap();

        assert!(docs.is_symlink());
        assert_eq!(roots, [docs.join("CLAUDE.md"), other.join("AGENTS.md")]);
    }
}
