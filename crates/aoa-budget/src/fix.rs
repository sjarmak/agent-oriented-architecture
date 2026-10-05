use std::path::{Path, PathBuf};

use crate::boundary::{Boundary, Reached};
use crate::budget::{count_budget, Config, Verdict};
use crate::closure::{resolve_closure_within, resolve_contained_closure, Closure};
use crate::error::BudgetError;
use crate::path::normalize_path;
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
    let path = &normalize_path(path);
    let original = root_text(resolve_contained_closure(path, boundary)?, path)?;
    let encoder = target_encoder(target)?;

    let (archive_name, archive_path, archive_found) = contained_archive(path, boundary)?;

    let summary = summarize_under(&original, ceiling, &archive_name, |t| {
        count_tokens(&encoder, t)
    });

    let permissions = std::fs::metadata(path)
        .map_err(|source| BudgetError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    let read_from = file_behind(path, boundary)?;
    let aliased = archive_found
        .map(|found| is_the_file_at(&found, &read_from))
        .transpose()
        .map_err(|source| BudgetError::Io {
            path: archive_path.clone(),
            source,
        })?;
    if aliased == Some(true) {
        return Err(BudgetError::ArchiveIsRoot {
            path: path.to_path_buf(),
            archive: archive_path,
        });
    }
    replace(&archive_path, &original, permissions.clone())?;
    replace(&read_from, &summary, permissions)?;

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

#[cfg(unix)]
fn is_the_file_at(entry: &Path, file: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let identity =
        |path: &Path| std::fs::symlink_metadata(path).map(|found| (found.dev(), found.ino()));
    Ok(identity(entry)? == identity(file)?)
}

#[cfg(not(unix))]
fn is_the_file_at(entry: &Path, file: &Path) -> std::io::Result<bool> {
    Ok(std::fs::canonicalize(entry)? == std::fs::canonicalize(file)?)
}

fn replace(path: &Path, body: &str, permissions: std::fs::Permissions) -> Result<(), BudgetError> {
    let failed = |source| BudgetError::Io {
        path: path.to_path_buf(),
        source,
    };
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut fresh = tempfile::Builder::new()
        .prefix(".aoa-budget-")
        .tempfile_in(dir)
        .map_err(failed)?;
    std::io::Write::write_all(&mut fresh, body.as_bytes()).map_err(failed)?;
    fresh
        .as_file()
        .set_permissions(permissions)
        .map_err(failed)?;
    fresh
        .persist(path)
        .map(drop)
        .map_err(|refused| failed(refused.error))
}

fn contained_archive(
    path: &Path,
    boundary: &Path,
) -> Result<(String, PathBuf, Option<PathBuf>), BudgetError> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "context".to_string());
    let dir = path.parent().unwrap_or(Path::new("."));
    let archive_name = format!("{stem}.archive.md");
    let archive_path = dir.join(&archive_name);
    let opened = Boundary::open(boundary).map_err(|source| BudgetError::Io {
        path: boundary.to_path_buf(),
        source,
    })?;
    let reached = opened
        .reach(&archive_path)
        .map_err(|source| BudgetError::Io {
            path: archive_path.clone(),
            source,
        })?;
    match reached {
        Reached::Outside => Err(BudgetError::OutsideBoundary {
            path: archive_path,
            boundary: boundary.to_path_buf(),
        }),
        Reached::Absent { .. } | Reached::Looping { .. } => Ok((archive_name, archive_path, None)),
        Reached::Member { entry, .. } => Ok((archive_name, archive_path, Some(entry))),
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
    use std::collections::BTreeSet;

    use super::*;

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
