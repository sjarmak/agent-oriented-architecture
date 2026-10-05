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

    let (archive_name, archive_path) = contained_archive(path, boundary)?;

    let summary = summarize_under(&original, ceiling, &archive_name, |t| {
        count_tokens(&encoder, t)
    });

    write_without_following_a_link(&archive_path, &original)?;
    std::fs::write(path, &summary).map_err(|source| BudgetError::Io {
        path: path.to_path_buf(),
        source,
    })?;

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

fn write_without_following_a_link(path: &Path, body: &str) -> Result<(), BudgetError> {
    let failed = |source| BudgetError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        rustix::fs::OFlags::NOFOLLOW.bits() as i32,
    );
    let mut archive = options.open(path).map_err(failed)?;
    let links = names_of(&archive).map_err(failed)?;
    if links > 1 {
        return Err(BudgetError::ArchiveHardLinked {
            path: path.to_path_buf(),
            links,
        });
    }
    archive.set_len(0).map_err(failed)?;
    std::io::Write::write_all(&mut archive, body.as_bytes()).map_err(failed)
}

#[cfg(unix)]
fn names_of(file: &std::fs::File) -> std::io::Result<u64> {
    Ok(std::os::unix::fs::MetadataExt::nlink(&file.metadata()?))
}

#[cfg(not(unix))]
fn names_of(_file: &std::fs::File) -> std::io::Result<u64> {
    Ok(1)
}

fn contained_archive(path: &Path, boundary: &Path) -> Result<(String, PathBuf), BudgetError> {
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
        Reached::Absent { .. } | Reached::Looping { .. } | Reached::Member { .. } => {
            Ok((archive_name, archive_path))
        }
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
