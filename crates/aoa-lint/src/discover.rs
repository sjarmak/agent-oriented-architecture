use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::Metadata;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use aoa_budget::{
    normalize_path, BudgetError, Closure, Directory, Enclosure, Entry, EntryKind, Opened,
};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::LintError;

const CONTEXT_ROOT_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const IGNORE_FILE_NAMES: [&str; 2] = [".gitignore", ".ignore"];
const UTF8_BOM: char = '\u{feff}';
const MAX_IGNORE_FILE_BYTES: u64 = 1 << 20;

pub struct LintedDirectory {
    enclosure: Enclosure,
}

pub struct DiscoveredRoot {
    path: PathBuf,
    text: String,
}

impl DiscoveredRoot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

impl LintedDirectory {
    pub fn hold(dir: &Path) -> Result<Self, LintError> {
        let enclosure = Enclosure::open(dir).map_err(walk_refused)?;
        Ok(Self { enclosure })
    }

    pub fn path(&self) -> &Path {
        self.enclosure.path()
    }

    pub fn discover(&self) -> Result<Vec<DiscoveredRoot>, LintError> {
        self.discover_checking(&|_| ())
    }

    pub(crate) fn resolve_discovered(&self, root: &DiscoveredRoot) -> Result<Closure, BudgetError> {
        self.enclosure.resolve_read(&root.path, &root.text)
    }

    fn discover_checking(&self, checked: &dyn Fn(&Path)) -> Result<Vec<DiscoveredRoot>, LintError> {
        let mut rules = Rules::for_linting(self, checked);
        let mut roots = Vec::new();
        let held = self.enclosure.hold().map_err(walk_refused)?;
        rules.enter(&held)?;
        let mut walking = vec![entries_of(held)?];
        while let Some((directory, entries)) = walking.last_mut() {
            let Some(entry) = entries.next() else {
                walking.pop();
                continue;
            };
            let path = directory.path().join(&entry.name);
            match entry.kind {
                EntryKind::Directory if entry.name != ".git" && rules.admit(&path, true) => {
                    let child = directory
                        .descend(&entry.name)
                        .map_err(|source| walk_failed(&path, source))?;
                    rules.enter(&child)?;
                    walking.push(entries_of(child)?);
                }
                EntryKind::File if is_context_root_name(&path) && rules.admit(&path, false) => {
                    let text = self.enclosure.read_held(&path, directory, &entry.name)?;
                    roots.push(DiscoveredRoot {
                        path: normalize_path(&path),
                        text,
                    });
                }
                EntryKind::Directory | EntryKind::File | EntryKind::Other => {}
            }
        }
        Ok(roots)
    }
}

type Walking = (Directory, std::vec::IntoIter<Entry>);

fn entries_of(directory: Directory) -> Result<Walking, LintError> {
    let entries = directory
        .entries()
        .map_err(|source| walk_failed(directory.path(), source))?;
    Ok((directory, entries.into_iter()))
}

fn walk_failed(path: &Path, source: io::Error) -> LintError {
    LintError::Walk {
        dir: path.to_path_buf(),
        source: source.into(),
    }
}

fn walk_refused(refused: BudgetError) -> LintError {
    match refused {
        BudgetError::Io { path, source } => walk_failed(&path, source),
        other => LintError::Budget(other),
    }
}

struct Rules<'a> {
    linted: &'a LintedDirectory,
    matchers: BTreeMap<PathBuf, Gitignore>,
    checked: &'a dyn Fn(&Path),
}

impl<'a> Rules<'a> {
    fn for_linting(linted: &'a LintedDirectory, checked: &'a dyn Fn(&Path)) -> Self {
        Self {
            linted,
            matchers: BTreeMap::new(),
            checked,
        }
    }

    fn enter(&mut self, directory: &Directory) -> Result<(), LintError> {
        let dir = directory.path();
        let mut builder = GitignoreBuilder::new(dir);
        for name in IGNORE_FILE_NAMES {
            if let Some(text) = self.read_ignore_file(directory, name)? {
                add_lines(&mut builder, &dir.join(name), &text)?;
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
            if ancestor == self.linted.path() {
                break;
            }
        }
        true
    }

    fn read_ignore_file(
        &self,
        directory: &Directory,
        name: &str,
    ) -> Result<Option<String>, LintError> {
        let path = directory.path().join(name);
        let Some(file) = self.open(directory, name)? else {
            return Ok(None);
        };
        let found = file
            .metadata()
            .map_err(|source| walk_failed(&path, source))?;
        if found.is_dir() {
            return Err(walk_failed(&path, io::ErrorKind::IsADirectory.into()));
        }
        if !found.is_file() {
            return Err(LintError::IgnoreFileNotRegular { path });
        }
        if has_another_name(&found) {
            return Err(LintError::IgnoreFileHardLinked { path });
        }
        (self.checked)(&path);
        let mut bytes = Vec::new();
        file.take(MAX_IGNORE_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| walk_failed(&path, source))?;
        if bytes.len() as u64 > MAX_IGNORE_FILE_BYTES {
            return Err(LintError::IgnoreFileOversized {
                path,
                max_bytes: MAX_IGNORE_FILE_BYTES,
            });
        }
        String::from_utf8(bytes).map(Some).map_err(|invalid| {
            walk_failed(
                &path,
                io::Error::new(io::ErrorKind::InvalidData, invalid.utf8_error()),
            )
        })
    }

    fn open(&self, directory: &Directory, name: &str) -> Result<Option<std::fs::File>, LintError> {
        let path = directory.path().join(name);
        let opened = directory
            .open_entry(OsStr::new(name))
            .map_err(|source| walk_failed(&path, source))?;
        match opened {
            Opened::File(file) => Ok(Some(file)),
            Opened::Absent => Ok(None),
            Opened::Link => self.open_link(&path),
        }
    }

    fn open_link(&self, path: &Path) -> Result<Option<std::fs::File>, LintError> {
        self.linted
            .enclosure
            .open_file(path)
            .map_err(|refused| match refused {
                BudgetError::OutsideBoundary { path, .. } => LintError::IgnoreFileOutside {
                    path,
                    dir: self.linted.path().to_path_buf(),
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

    fn paths_of(roots: Vec<DiscoveredRoot>) -> Vec<PathBuf> {
        roots.into_iter().map(DiscoveredRoot::into_path).collect()
    }

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

        let roots = paths_of(
            LintedDirectory::hold(&repo)
                .unwrap()
                .discover_checking(&swapping)
                .unwrap(),
        );

        assert!(docs.is_symlink());
        assert_eq!(roots, [docs.join("CLAUDE.md"), other.join("AGENTS.md")]);
    }

    #[test]
    fn a_directory_swapped_for_a_link_after_its_rules_were_checked_lists_what_was_held_not_what_the_link_reaches(
    ) {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let repo = base.join("repo");
        let docs = repo.join("docs");
        let outside = base.join("outside");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(docs.join("CLAUDE.md"), "").unwrap();
        std::fs::write(docs.join(".gitignore"), "vendor/\n").unwrap();
        std::fs::write(docs.join(".ignore"), "build/\n").unwrap();
        std::fs::write(outside.join("AGENTS.md"), "").unwrap();
        let swapping = |checked: &Path| {
            if checked == docs.join(".ignore") {
                std::fs::rename(&docs, &elsewhere).unwrap();
                std::os::unix::fs::symlink(&outside, &docs).unwrap();
            }
        };

        let roots = paths_of(
            LintedDirectory::hold(&repo)
                .unwrap()
                .discover_checking(&swapping)
                .unwrap(),
        );

        assert!(docs.is_symlink());
        assert!(docs.join("AGENTS.md").is_file());
        assert_eq!(roots, [docs.join("CLAUDE.md")]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn discovered_roots_hold_no_descriptor_on_the_directories_that_contain_them() {
        let base = tempfile::tempdir().unwrap();
        let repo = base.path().canonicalize().unwrap();
        for index in 0..64 {
            let package = repo.join(format!("package-{index}"));
            std::fs::create_dir(&package).unwrap();
            std::fs::write(package.join("CLAUDE.md"), "").unwrap();
        }
        let linted = LintedDirectory::hold(&repo).unwrap();
        let held_before = descriptors_under(&repo);

        let roots = linted.discover().unwrap();

        assert_eq!(descriptors_under(&repo), held_before);
        assert_eq!(roots.len(), 64);
    }

    #[cfg(target_os = "linux")]
    fn descriptors_under(dir: &Path) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|entry| std::fs::read_link(entry.unwrap().path()).ok())
            .filter(|target| target.starts_with(dir))
            .count()
    }
}
