use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::boundary::{Boundary, Reached};
use crate::budget::{count_budget, Config, Verdict};
use crate::closure::{open_boundary, resolve_closure_held, resolve_contained_closure, Closure};
use crate::directory::{Directory, Placement};
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
/// tail until the summarized file counts under `ceiling`.
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

    let opened = open_boundary(boundary)?;
    let read_from = reach_member(&opened, path, path, boundary)?;
    let (root_directory, root_name) = opened
        .split(&read_from)
        .map_err(|source| io_at(path, source))?;
    let archive_name = archive_name_of(path.file_name().unwrap_or(&root_name), path)?;
    let archive_path = root_directory.join(&archive_name);
    let summary = summarize_under(&original, ceiling, &archive_name, |t| {
        count_tokens(&encoder, t)
    });

    let within = hold_directory_of(&opened, &root_directory, path, boundary)?;
    let archive_name = OsStr::new(&archive_name);
    if within
        .holds(archive_name)
        .map_err(|source| io_at(&archive_path, source))?
    {
        return Err(BudgetError::ArchiveExists {
            path: path.to_path_buf(),
            archive: archive_path,
        });
    }
    let root_standing = Standing::of_writable(&within, &root_name)
        .and_then(|standing| standing.ok_or_else(|| std::io::ErrorKind::NotFound.into()))
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::PermissionDenied => BudgetError::RootNotWritable {
                path: path.to_path_buf(),
                source,
            },
            _ => io_at(path, source),
        })?;
    let archive = Replacement::prepare(
        &within,
        archive_name,
        Placement::Creating,
        &original,
        &root_standing.of_a_new_file(),
    )
    .map_err(|unplaced| unplaced.at(&archive_path))?;
    let root = match Replacement::prepare(
        &within,
        &root_name,
        Placement::Replacing,
        &summary,
        &root_standing,
    ) {
        Ok(root) => root,
        Err(unplaced) => return Err(unplaced.withdrawing(archive).at(&read_from)),
    };
    if let Err(unplaced) = archive.put(placing) {
        return Err(unplaced.withdrawing(root).at(&archive_path));
    }
    root.put(placing)
        .map_err(|unplaced| unplaced.at(&read_from))?;

    let closure = resolve_closure_held(path, &opened, &within, &root_name)?;
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

fn io_at(path: &Path, source: std::io::Error) -> BudgetError {
    BudgetError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn reach_member(
    opened: &Boundary,
    target: &Path,
    path: &Path,
    boundary: &Path,
) -> Result<PathBuf, BudgetError> {
    let reached = opened.reach(target).map_err(|source| io_at(path, source))?;
    match reached {
        Reached::Member { resolved, .. } => Ok(resolved),
        Reached::Outside => Err(BudgetError::OutsideBoundary {
            path: path.to_path_buf(),
            boundary: boundary.to_path_buf(),
        }),
        Reached::Absent { .. } | Reached::Looping { .. } => {
            Err(io_at(path, std::io::ErrorKind::NotFound.into()))
        }
    }
}

fn hold_directory_of(
    opened: &Boundary,
    directory: &Path,
    path: &Path,
    boundary: &Path,
) -> Result<Directory, BudgetError> {
    let resolved = reach_member(opened, directory, path, boundary)?;
    opened
        .hold_directory(&resolved)
        .map_err(|source| io_at(path, source))
}

fn archive_name_of(name: &OsStr, path: &Path) -> Result<String, BudgetError> {
    let stem = Path::new(name).file_stem().unwrap_or(name);
    let stem = stem
        .to_str()
        .ok_or_else(|| BudgetError::ArchiveNameNotUtf8 {
            path: path.to_path_buf(),
        })?;
    Ok(format!("{stem}.archive.md"))
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

    fn prepared_root_beside(dir: &Path, archive_temp: &OsStr) -> PathBuf {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|entry| entry.file_name() != Some(archive_temp) && !entry.ends_with("big.md"))
            .unwrap()
    }

    fn strand(temp: &Path) {
        std::fs::remove_file(temp).unwrap();
        std::fs::create_dir(temp).unwrap();
        std::fs::write(temp.join("held"), "").unwrap();
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
            rename: &|directory, from, to, placement| {
                if to == "big.md" {
                    return Err(refused());
                }
                (Placing::REAL.rename)(directory, from, to, placement)
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
            sync_directory: &|directory| {
                synced.set(synced.get() + 1);
                if synced.get() == 1 {
                    return Err(refused());
                }
                (Placing::REAL.sync_directory)(directory)
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
            rename: &|_directory, from, _to, _placement| {
                let prepared_root = prepared_root_beside(tree.dir.path(), from);
                strand(&prepared_root);
                stranded.set(Some(prepared_root));
                Err(refused())
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

    #[test]
    fn two_temporary_files_that_cannot_be_withdrawn_are_both_named_when_the_archive_is_refused() {
        let tree = tree();
        let stranded = Cell::new(None);
        let stranding_both = Placing {
            rename: &|directory, from, _to, _placement| {
                let prepared_root = prepared_root_beside(tree.dir.path(), from);
                let archive_temp = directory.naming(from);
                strand(&prepared_root);
                strand(&archive_temp);
                stranded.set(Some((archive_temp, prepared_root)));
                Err(refused())
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
    fn an_entry_made_at_the_archive_name_after_the_check_is_kept_and_nothing_is_replaced() {
        let tree = tree();
        let planting = Placing {
            rename: &|directory, from, to, placement| {
                std::fs::write(&tree.archive, "planted").unwrap();
                (Placing::REAL.rename)(directory, from, to, placement)
            },
            sync_directory: Placing::REAL.sync_directory,
        };

        let refused = fix(&tree, &planting);

        assert!(
            matches!(
                &refused,
                Err(BudgetError::Io { path, source })
                    if path == &tree.archive && source.kind() == std::io::ErrorKind::AlreadyExists
            ),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_to_string(&tree.archive).unwrap(), "planted");
        assert_eq!(
            std::fs::read_to_string(&tree.root).unwrap(),
            oversized_body()
        );
        assert_eq!(names_in(tree.dir.path()), ["big.archive.md", "big.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_swapped_for_a_link_while_placing_writes_nothing_where_the_link_points() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let repo = base.join("repo");
        let docs = repo.join("docs");
        let outside = base.join("outside");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("big.md"), "outside\n").unwrap();
        let root = docs.join("big.md");
        std::fs::write(&root, oversized_body()).unwrap();
        let swapped = Cell::new(false);
        let swapping = Placing {
            rename: &|directory, from, to, placement| {
                if !swapped.replace(true) {
                    std::fs::rename(&docs, &elsewhere).unwrap();
                    std::os::unix::fs::symlink(&outside, &docs).unwrap();
                }
                (Placing::REAL.rename)(directory, from, to, placement)
            },
            sync_directory: Placing::REAL.sync_directory,
        };

        let outcome = fix_placing(&root, &repo, 200, "gpt-4o", &swapping).unwrap();

        assert!(swapped.get());
        assert_eq!(names_in(&outside), ["big.md"]);
        assert_eq!(
            std::fs::read_to_string(outside.join("big.md")).unwrap(),
            "outside\n"
        );
        assert_eq!(names_in(&elsewhere), ["big.archive.md", "big.md"]);
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("big.archive.md")).unwrap(),
            oversized_body()
        );
        let summary = std::fs::read_to_string(elsewhere.join("big.md")).unwrap();
        assert!(summary.starts_with("> Summarized to fit budget."));
        let encoder = target_encoder("gpt-4o").unwrap();
        assert_ne!(
            count_tokens(&encoder, &summary),
            count_tokens(&encoder, "outside\n")
        );
        assert_eq!(outcome.target_tokens, count_tokens(&encoder, &summary));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_context_file_is_archived_beside_the_file_the_link_names() {
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let repo = base.join("repo");
        let docs = repo.join("docs");
        let real = repo.join("real");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("big.md"), oversized_body()).unwrap();
        let link = docs.join("link.md");
        std::os::unix::fs::symlink("../real/big.md", &link).unwrap();

        let outcome = fix_placing(&link, &repo, 200, "gpt-4o", &Placing::REAL).unwrap();

        assert_eq!(outcome.root, link);
        assert_eq!(outcome.archive, real.join("link.archive.md"));
        assert_eq!(names_in(&docs), ["link.md"]);
        assert_eq!(names_in(&real), ["big.md", "link.archive.md"]);
        assert_eq!(
            std::fs::read_to_string(real.join("link.archive.md")).unwrap(),
            oversized_body()
        );
        let summary = std::fs::read_to_string(&link).unwrap();
        assert!(summary.contains("[link.archive.md]"), "{summary}");
    }

    #[cfg(unix)]
    #[test]
    fn a_context_file_whose_name_is_not_utf8_is_refused_before_anything_is_written() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let name = OsStr::from_bytes(b"big-\xff.md");
        let root = dir.path().join(name);
        std::fs::write(&root, oversized_body()).unwrap();

        let refused = fix_placing(&root, dir.path(), 200, "gpt-4o", &Placing::REAL);

        assert!(
            matches!(&refused, Err(BudgetError::ArchiveNameNotUtf8 { path }) if path == &root),
            "{refused:?}"
        );
        assert_eq!(names_in(dir.path()), [name]);
        assert_eq!(std::fs::read_to_string(&root).unwrap(), oversized_body());
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
