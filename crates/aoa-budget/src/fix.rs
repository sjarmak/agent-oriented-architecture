use std::path::{Path, PathBuf};

use crate::boundary::{Boundary, Reached};
use crate::budget::{count_budget, Config, Verdict};
use crate::closure::{resolve_closure_within, resolve_contained_closure, Closure};
use crate::error::BudgetError;
use crate::path::normalize_path;
use crate::replace::{Placing, Replacement, Standing};
use crate::tokenizer::{count_tokens, target_encoder};

/// The outcome of a [`fix_oversized`] operation.
#[derive(Debug, Clone)]
pub struct FixOutcome {
    /// The file that was rewritten to an under-budget summary.
    pub root: PathBuf,
    /// Where the full original body was archived for retrieval on demand.
    pub archive: PathBuf,
    /// Target-token count of the re-resolved closure after the fix.
    pub target_tokens: usize,
}

/// Bring an over-budget file under the ceiling by extractive summarization.
///
/// The body is reduced deterministically (no external model): every markdown
/// heading is kept verbatim, and each remaining paragraph is condensed to its
/// first line followed by an elision marker. Paragraphs are dropped from the
/// tail until the summarized file counts under `ceiling`. The full original
/// body is archived to a sibling `<stem>.archive.md`, which the summary points
/// to but does **not** link (it is reference material loaded on demand, so it
/// stays out of the active context closure).
///
/// After writing, the closure rooted at `path` is re-resolved and re-counted;
/// if it is still at or over the ceiling, [`BudgetError::FixFailed`] is
/// returned rather than reporting a false green.
pub fn fix_oversized(
    path: &Path,
    boundary: &Path,
    ceiling: usize,
    target: &str,
) -> Result<FixOutcome, BudgetError> {
    fix_placing(path, boundary, ceiling, target, &Placing::REAL)
}

fn fix_placing(
    path: &Path,
    boundary: &Path,
    ceiling: usize,
    target: &str,
    placing: &Placing,
) -> Result<FixOutcome, BudgetError> {
    let path = &normalize_path(path);
    let original = root_text(resolve_contained_closure(path, boundary)?, path)?;
    let encoder = target_encoder(target)?;

    let (archive_name, archive_path) = free_archive_name(path)?;

    let summary = summarize_under(&original, ceiling, &archive_name, |t| {
        count_tokens(&encoder, t)
    });

    let read_from = file_behind(path, boundary)?;
    let root_standing = Standing::of_writable(&read_from)
        .and_then(|standing| standing.ok_or_else(|| std::io::ErrorKind::NotFound.into()))
        .map_err(|source| BudgetError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let archive = Replacement::prepare(&archive_path, &original, &root_standing.of_a_new_file())
        .map_err(|unplaced| unplaced.at(&archive_path))?;
    let root = match Replacement::prepare(&read_from, &summary, &root_standing) {
        Ok(root) => root,
        Err(unplaced) => return Err(unplaced.withdrawing(archive).at(&read_from)),
    };
    if let Err(unplaced) = archive.put(placing) {
        return Err(unplaced.withdrawing(root).at(&archive_path));
    }
    root.put(placing)
        .map_err(|unplaced| unplaced.at(&read_from))?;

    let closure = resolve_closure_within(path, boundary)?;
    let report = count_budget(&closure, target, &Config::blocking(ceiling))?;
    if report.verdict != Verdict::Pass {
        return Err(BudgetError::FixFailed {
            target_tokens: report.gating_target_tokens,
            ceiling,
        });
    }

    Ok(FixOutcome {
        root: path.to_path_buf(),
        archive: archive_path,
        target_tokens: report.gating_target_tokens,
    })
}

fn root_text(closure: Closure, path: &Path) -> Result<String, BudgetError> {
    closure
        .files
        .into_iter()
        .next()
        .map(|root| root.text)
        .ok_or_else(|| BudgetError::RootNotRead {
            path: path.to_path_buf(),
        })
}

fn file_behind(path: &Path, boundary: &Path) -> Result<PathBuf, BudgetError> {
    let opened = Boundary::open(boundary).map_err(|source| BudgetError::Io {
        path: boundary.to_path_buf(),
        source,
    })?;
    let reached = opened.reach(path).map_err(|source| BudgetError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    match reached {
        Reached::Member { resolved, .. } => Ok(resolved),
        Reached::Outside => Err(BudgetError::OutsideBoundary {
            path: path.to_path_buf(),
            boundary: boundary.to_path_buf(),
        }),
        Reached::Absent { .. } | Reached::Looping { .. } => Err(BudgetError::Io {
            path: path.to_path_buf(),
            source: std::io::ErrorKind::NotFound.into(),
        }),
    }
}

fn free_archive_name(path: &Path) -> Result<(String, PathBuf), BudgetError> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "context".to_string());
    let dir = path.parent().unwrap_or(Path::new("."));
    let archive_name = format!("{stem}.archive.md");
    let archive_path = dir.join(&archive_name);
    match std::fs::symlink_metadata(&archive_path) {
        Ok(_) => Err(BudgetError::ArchiveExists {
            path: path.to_path_buf(),
            archive: archive_path,
        }),
        Err(absent) if absent.kind() == std::io::ErrorKind::NotFound => {
            Ok((archive_name, archive_path))
        }
        Err(source) => Err(BudgetError::Io {
            path: archive_path,
            source,
        }),
    }
}

/// Build an extractive summary that counts under `ceiling`.
///
/// Headings (`#`-prefixed lines) are kept verbatim; every other paragraph is
/// reduced to its first line plus an elision marker. Paragraphs are appended in
/// order and the build stops before adding one would reach the ceiling, so the
/// result is always strictly under it (assuming the header alone fits).
fn summarize_under(
    text: &str,
    ceiling: usize,
    archive_name: &str,
    count: impl Fn(&str) -> usize,
) -> String {
    let header = format!("> Summarized to fit budget. Full text: [{archive_name}]\n");
    let mut out = header.clone();

    for para in text.split("\n\n") {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            continue;
        }
        let condensed = if trimmed.starts_with('#') {
            format!("{trimmed}\n")
        } else {
            let first = trimmed.lines().next().unwrap_or("");
            format!("{first} …\n")
        };
        let candidate = format!("{out}\n{condensed}");
        if count(&candidate) >= ceiling {
            break;
        }
        out = candidate;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::ffi::OsString;

    use tempfile::PersistError;

    use super::*;

    struct Tree {
        dir: tempfile::TempDir,
        root: PathBuf,
        archive: PathBuf,
    }

    fn oversized_body() -> String {
        "A paragraph of guidance that every package must follow.\n\n".repeat(200)
    }

    fn tree() -> Tree {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("big.md");
        std::fs::write(&root, oversized_body()).unwrap();
        let archive = dir.path().join("big.archive.md");
        Tree { dir, root, archive }
    }

    fn fix(tree: &Tree, placing: &Placing) -> Result<FixOutcome, BudgetError> {
        fix_placing(&tree.root, tree.dir.path(), 200, "gpt-4o", placing)
    }

    fn names_in(dir: &Path) -> Vec<OsString> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    fn refused() -> std::io::Error {
        std::io::Error::other("refused by the test")
    }

    fn is_named(path: &Path, name: &str) -> bool {
        path.file_name().is_some_and(|found| found == name)
    }

    fn assert_nothing_was_lost_and_a_rerun_is_refused(
        tree: &Tree,
        interrupted: Result<FixOutcome, BudgetError>,
        stopped_at: &str,
    ) {
        assert!(
            matches!(&interrupted, Err(BudgetError::Io { path, .. }) if is_named(path, stopped_at)),
            "{interrupted:?}"
        );
        let body = oversized_body();
        assert_eq!(std::fs::read_to_string(&tree.root).unwrap(), body);
        assert_eq!(std::fs::read_to_string(&tree.archive).unwrap(), body);
        assert_eq!(names_in(tree.dir.path()), ["big.archive.md", "big.md"]);

        let rerun = fix(tree, &Placing::REAL);

        assert!(
            matches!(
                &rerun,
                Err(BudgetError::ArchiveExists { path, archive })
                    if path == &tree.root && archive == &tree.archive
            ),
            "{rerun:?}"
        );
        assert_eq!(std::fs::read_to_string(&tree.root).unwrap(), body);
        assert_eq!(std::fs::read_to_string(&tree.archive).unwrap(), body);
        assert_eq!(names_in(tree.dir.path()), ["big.archive.md", "big.md"]);
    }

    #[test]
    fn a_root_that_cannot_be_placed_after_its_archive_loses_nothing_and_a_rerun_is_refused() {
        let tree = tree();
        let failing_the_root = Placing {
            rename: &|fresh, to| {
                if is_named(to, "big.md") {
                    return Err(PersistError {
                        error: refused(),
                        file: fresh,
                    });
                }
                (Placing::REAL.rename)(fresh, to)
            },
            sync_directory: Placing::REAL.sync_directory,
        };

        let interrupted = fix(&tree, &failing_the_root);

        assert_nothing_was_lost_and_a_rerun_is_refused(&tree, interrupted, "big.md");
    }

    #[test]
    fn an_archive_directory_that_cannot_be_synced_loses_nothing_and_a_rerun_is_refused() {
        let tree = tree();
        let synced = Cell::new(0);
        let failing_the_first_sync = Placing {
            rename: Placing::REAL.rename,
            sync_directory: &|dir| {
                synced.set(synced.get() + 1);
                if synced.get() == 1 {
                    return Err(refused());
                }
                (Placing::REAL.sync_directory)(dir)
            },
        };

        let interrupted = fix(&tree, &failing_the_first_sync);

        assert_eq!(synced.get(), 1);
        assert_nothing_was_lost_and_a_rerun_is_refused(&tree, interrupted, "big.archive.md");
    }

    #[test]
    fn a_prepared_root_that_cannot_be_withdrawn_is_named_when_the_archive_is_refused() {
        let tree = tree();
        let stranded = Cell::new(None);
        let refusing_the_archive = Placing {
            rename: &|fresh, _to| {
                let prepared_root = std::fs::read_dir(tree.dir.path())
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|entry| entry != fresh.path() && !entry.ends_with("big.md"))
                    .unwrap();
                std::fs::remove_file(&prepared_root).unwrap();
                std::fs::create_dir(&prepared_root).unwrap();
                std::fs::write(prepared_root.join("held"), "").unwrap();
                stranded.set(Some(prepared_root));
                Err(PersistError {
                    error: refused(),
                    file: fresh,
                })
            },
            sync_directory: Placing::REAL.sync_directory,
        };

        let refused = fix(&tree, &refusing_the_archive);

        let stranded = stranded.take().unwrap();
        assert!(
            matches!(
                &refused,
                Err(BudgetError::TempFileLeftBehind { path, left, .. })
                    if path == &tree.archive
                        && matches!(left.as_slice(), [one] if one.temp == stranded)
            ),
            "{refused:?}"
        );
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains(&stranded.display().to_string()));
        assert_eq!(
            std::fs::read_to_string(&tree.root).unwrap(),
            oversized_body()
        );
    }

    fn strand(temp: &Path) {
        std::fs::remove_file(temp).unwrap();
        std::fs::create_dir(temp).unwrap();
        std::fs::write(temp.join("held"), "").unwrap();
    }

    #[test]
    fn two_temporary_files_that_cannot_be_withdrawn_are_both_named_when_the_archive_is_refused() {
        let tree = tree();
        let stranded = Cell::new(None);
        let stranding_both = Placing {
            rename: &|fresh, _to| {
                let prepared_root = std::fs::read_dir(tree.dir.path())
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|entry| entry != fresh.path() && !entry.ends_with("big.md"))
                    .unwrap();
                strand(&prepared_root);
                strand(fresh.path());
                stranded.set(Some((fresh.path().to_path_buf(), prepared_root)));
                Err(PersistError {
                    error: refused(),
                    file: fresh,
                })
            },
            sync_directory: Placing::REAL.sync_directory,
        };

        let refused = fix(&tree, &stranding_both);

        let (archive_temp, root_temp) = stranded.take().unwrap();
        assert!(
            matches!(&refused, Err(BudgetError::TempFileLeftBehind { path, .. }) if path == &tree.archive),
            "{refused:?}"
        );
        let reported = refused.unwrap_err().to_string();
        for temp in [&archive_temp, &root_temp] {
            assert!(
                reported.contains(&temp.display().to_string()),
                "{temp:?} is left behind but not named in: {reported}"
            );
            assert!(temp.join("held").exists());
        }
        assert_eq!(
            std::fs::read_to_string(&tree.root).unwrap(),
            oversized_body()
        );
    }

    #[test]
    fn a_closure_without_its_root_is_an_error_and_not_an_empty_body() {
        let path = Path::new("AGENTS.md");
        let closure = Closure {
            root: path.to_path_buf(),
            files: Vec::new(),
            outside_boundary: Vec::new(),
            unread: Vec::new(),
            absent: BTreeSet::new(),
        };

        let refused = root_text(closure, path);

        assert!(
            matches!(&refused, Err(BudgetError::RootNotRead { path: named }) if named == path),
            "{refused:?}"
        );
    }
}
