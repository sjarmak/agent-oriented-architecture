//! Dead-import migration — a shared engine plus per-language adapters behind the
//! [`ImportAdapter`] trait.
//!
//! The single generic [`DeadImportFix`] is a [`CodeFix`] that drives a language
//! adapter through one engine. The engine owns everything that must be identical
//! across languages — and where a path-safety or subtractivity invariant lives:
//!
//! 1. Honest-degrade to an empty plan when the adapter's root marker is absent
//!    (a non-matching tree is *ineligible*, not an error).
//! 2. Copy the repo to an isolated [`TempDir`](tempfile::TempDir), excluding build
//!    output and VCS metadata, so the real checkout is never written and the tool
//!    runs on a throwaway tree. Dropped when planning returns — no residue.
//! 3. Ask the adapter for the strictly-subtractive rewrite of each touched file
//!    (the adapter runs its tool, restricted to the ONE unused-import lint class).
//! 4. Map each isolated-copy path back to the real repo *exactly* and emit an
//!    [`Overwrite`](crate::ChangeAction::Overwrite) [`PlannedChange`].
//!
//! An adapter only ever returns [`SubtractedFile`]s (a tree-relative path plus the
//! file's post-fix content). It is structurally incapable of constructing a
//! [`PlannedChange`] or an absolute real-repo path, so the temp→real round-trip
//! and the repo-containment guard stay in one reviewed place ([`finalize_changes`])
//! and cannot diverge per language.
//!
//! Construct-validity boundary: "subtractive" is not the same as "correct". Each
//! adapter declares its own (often unchecked) eligibility preconditions — the Rust
//! cfg-gated-import case, the Python `TYPE_CHECKING` case — rather than pretending
//! to validate them. See `docs/r0_runbook.md`.

mod python;
mod rust;
mod typescript;

use std::path::{Path, PathBuf};

use crate::error::MigrateError;
use crate::fix::{ChangeAction, CodeFix, FixProvenance, PlannedChange};

/// Directory basenames excluded from the isolated copy, at any nesting depth:
/// build output (`target`, `build`, `dist`), VCS metadata (`.git`), this tool's
/// own bookkeeping (`.aoa`), Python/JS caches and vendored trees (`node_modules`,
/// `__pycache__`, `.venv`, `venv`). Excluding nested `target/` matters for
/// workspaces with per-member build dirs.
pub(crate) const COPY_EXCLUDE_DIRS: &[&str] = &[
    "target",
    ".git",
    ".aoa",
    "node_modules",
    "build",
    "dist",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".ruff_cache",
];

/// One file an adapter would rewrite in the isolated tree: its path **relative to
/// the isolated copy root** plus its content **after** the strictly-subtractive
/// removal. The engine maps the relative path back to the real repo. Carrying
/// only a relative path (never an absolute one) is what keeps every adapter
/// inside the repo-containment guard in [`finalize_changes`].
pub(crate) struct SubtractedFile {
    pub rel_path: PathBuf,
    pub new_content: String,
}

/// One language's dead-import treatment. The engine ([`DeadImportFix`]) owns
/// isolation, the temp→real path round-trip, repo-containment validation,
/// deterministic ordering, and honest-degrade. An adapter owns ONLY: root-marker
/// probe, tool invocation restricted to the ONE unused-import lint class,
/// strictly-subtractive edit production, provenance, and tool-failure
/// classification into the shared [`MigrateError`] taxonomy.
pub(crate) trait ImportAdapter: Send + Sync {
    /// Stable CLI-selectable fix id, e.g. `dead-imports` (Rust),
    /// `dead-imports-python`.
    fn id(&self) -> &'static str;

    /// One-line human description.
    fn describe(&self) -> &'static str;

    /// The R0 eligibility precondition recorded in the manifest.
    fn eligibility_note(&self) -> &'static str;

    /// Eligibility probe on the REAL repo: is this language's root marker present?
    /// `false` ⇒ the engine honest-degrades to an empty plan (never an error).
    fn is_eligible(&self, repo: &Path) -> bool;

    /// Run the tool against the ISOLATED copy `work` and return the strictly-
    /// subtractive rewrite of each touched file. The adapter MUST classify tool or
    /// build failure into the shared taxonomy (`ToolchainUnavailable` /
    /// `BuildFailed` / `RepoDoesNotCheck`) — LOUD, never a silent empty `Vec`. An
    /// empty `Vec` means "tool ran cleanly, nothing to remove" (legitimate).
    fn subtract_imports(&self, work: &Path) -> Result<Vec<SubtractedFile>, MigrateError>;

    /// Toolchain/version provenance, resolved against the real repo.
    fn provenance(&self, repo: &Path) -> Result<Option<FixProvenance>, MigrateError>;
}

/// The single dead-import [`CodeFix`]: drives a language [`ImportAdapter`] through
/// the shared engine. Construct one per language via [`DeadImportFix::rust`],
/// [`DeadImportFix::python`], or [`DeadImportFix::typescript`].
pub struct DeadImportFix {
    adapter: Box<dyn ImportAdapter>,
}

impl DeadImportFix {
    /// Rust adapter: rustc-certified `unused_imports` via an isolated `cargo check`
    /// + `rustfix`.
    pub fn rust() -> Self {
        Self {
            adapter: Box::new(rust::RustImportAdapter),
        }
    }

    /// Python adapter: ruff `F401` (unused-import) via an isolated, config-blind
    /// `ruff check --select F401 --fix`.
    pub fn python() -> Self {
        Self {
            adapter: Box::new(python::PythonImportAdapter),
        }
    }

    /// TypeScript/JS adapter: a vendored, pinned ESLint + `eslint-plugin-unused-imports`
    /// run hermetically against the isolated copy.
    pub fn typescript() -> Self {
        Self {
            adapter: Box::new(typescript::TsImportAdapter),
        }
    }
}

impl CodeFix for DeadImportFix {
    fn id(&self) -> &str {
        self.adapter.id()
    }

    fn describe(&self) -> &str {
        self.adapter.describe()
    }

    fn eligibility_note(&self) -> &str {
        self.adapter.eligibility_note()
    }

    fn plan(&self, repo: &Path) -> Result<Vec<PlannedChange>, MigrateError> {
        // (1) A tree without this language's root marker is ineligible, not broken.
        if !self.adapter.is_eligible(repo) {
            return Ok(Vec::new());
        }

        // (2) Isolate: copy to a throwaway tree so the tool never writes the real
        // checkout. Dropped at end of scope ⇒ no residue.
        let work = tempfile::Builder::new()
            .prefix("aoa-dead-imports-")
            .tempdir()
            .map_err(|source| MigrateError::Io {
                path: repo.to_path_buf(),
                source,
            })?;
        copy_tree(repo, work.path())?;

        // (3) Per-language strictly-subtractive rewrite of the isolated copy.
        let subtracted = self.adapter.subtract_imports(work.path())?;

        // (4) Shared temp→real round-trip + containment guard + Overwrite emission.
        finalize_changes(subtracted, work.path(), repo)
    }

    fn provenance(&self, repo: &Path) -> Result<Option<FixProvenance>, MigrateError> {
        self.adapter.provenance(repo)
    }
}

/// Recursively copy `src` into `dst`, skipping [`COPY_EXCLUDE_DIRS`] at any depth
/// and never following symlinks (a symlink is skipped rather than traversed, so
/// the copy stays within the source tree).
pub(crate) fn copy_tree(src: &Path, dst: &Path) -> Result<(), MigrateError> {
    std::fs::create_dir_all(dst).map_err(|source| io_err(dst, source))?;
    for entry in std::fs::read_dir(src).map_err(|source| io_err(src, source))? {
        let entry = entry.map_err(|source| io_err(src, source))?;
        let name = entry.file_name();
        let from = entry.path();
        let to = dst.join(&name);
        // `file_type` does not follow symlinks.
        let ft = entry.file_type().map_err(|source| io_err(&from, source))?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if COPY_EXCLUDE_DIRS.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            copy_tree(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|source| MigrateError::Copy {
                from: from.clone(),
                to: to.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Map each adapter-produced [`SubtractedFile`] back to the real repo and emit an
/// `Overwrite` change. This is the SINGLE place the temp→real round-trip and the
/// repo-containment guard live, so they cannot diverge per language:
///
/// - the relative path is canonicalized inside the isolated tree (rejecting any
///   path that escapes it — defense against a buggy adapter),
/// - the corresponding real-repo file must exist and canonicalize inside the repo,
/// - `old_content` is read from the REAL repo (what the diff preview and the
///   apply-time archive describe),
/// - a no-op rewrite (post-fix content equals the real content) is dropped.
fn finalize_changes(
    subtracted: Vec<SubtractedFile>,
    work: &Path,
    repo: &Path,
) -> Result<Vec<PlannedChange>, MigrateError> {
    // Canonicalize both ends once so the temp→real round-trip is exact even when
    // /tmp is a symlink or paths arrive non-canonical.
    let work_canon = std::fs::canonicalize(work).map_err(|source| io_err(work, source))?;
    let repo_canon = std::fs::canonicalize(repo).map_err(|source| io_err(repo, source))?;

    let mut changes = Vec::new();
    for SubtractedFile {
        rel_path,
        new_content,
    } in subtracted
    {
        // An adapter must only ever hand back a path relative to the isolated tree.
        // Anything that resolves outside the copied tree has no real-repo
        // equivalent; drop it rather than misroute a write.
        let Some(rel) = contained_relative(&rel_path, work, &work_canon) else {
            continue;
        };
        let real_file = repo.join(&rel);

        // Defense in depth before apply.rs ever sees the path: the real target
        // must exist and resolve inside the real repo.
        match std::fs::canonicalize(&real_file) {
            Ok(c) if c.starts_with(&repo_canon) => {}
            _ => continue,
        }

        let old_content =
            std::fs::read_to_string(&real_file).map_err(|source| io_err(&real_file, source))?;
        if new_content == old_content {
            continue;
        }
        changes.push(PlannedChange {
            path: real_file,
            action: ChangeAction::Overwrite,
            new_content,
            old_content: Some(old_content),
        });
    }

    // Deterministic order regardless of adapter iteration.
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Resolve an adapter's tree-relative path to a clean path relative to the
/// isolated copy root, returning `None` for anything that escapes the tree (an
/// absolute path, or a `..` traversal out of `work`).
fn contained_relative(rel_path: &Path, work: &Path, work_canon: &Path) -> Option<PathBuf> {
    if rel_path.is_absolute() {
        return None;
    }
    let abs_canon = std::fs::canonicalize(work.join(rel_path)).ok()?;
    abs_canon
        .strip_prefix(work_canon)
        .ok()
        .map(Path::to_path_buf)
}

pub(crate) fn io_err(path: &Path, source: std::io::Error) -> MigrateError {
    MigrateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Collect every regular file under `work` whose path satisfies `keep`, returned
/// as paths relative to `work` in deterministic (sorted) order. Symlinks are not
/// followed. Build/VCS dirs are already absent (the isolated copy excludes
/// [`COPY_EXCLUDE_DIRS`]), so a plain recursive walk is enough.
pub(crate) fn collect_files(
    work: &Path,
    keep: &dyn Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, MigrateError> {
    fn walk(
        dir: &Path,
        base: &Path,
        keep: &dyn Fn(&Path) -> bool,
        out: &mut Vec<PathBuf>,
    ) -> Result<(), MigrateError> {
        for entry in std::fs::read_dir(dir).map_err(|s| io_err(dir, s))? {
            let entry = entry.map_err(|s| io_err(dir, s))?;
            let path = entry.path();
            let ft = entry.file_type().map_err(|s| io_err(&path, s))?;
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&path, base, keep, out)?;
            } else if ft.is_file() && keep(&path) {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_path_buf());
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(work, work, keep, &mut out)?;
    out.sort();
    Ok(out)
}

/// Model-A subtraction: snapshot the eligible files under `work`, run the tool's
/// own in-place autofixer via `fix`, then return the files whose content changed.
/// Adapters that drive a tool's `--fix` (ruff, ESLint) share this so the
/// snapshot/diff lives in one reviewed place. The returned paths are tree-relative
/// (the engine maps them to the real repo); a file the tool only touches
/// cosmetically still surfaces here and is filtered by the per-tool single-lint
/// restriction upstream, then by [`finalize_changes`]' no-op guard downstream.
pub(crate) fn subtract_via_inplace_fix(
    work: &Path,
    keep: &dyn Fn(&Path) -> bool,
    fix: impl FnOnce(&Path, &[PathBuf]) -> Result<(), MigrateError>,
) -> Result<Vec<SubtractedFile>, MigrateError> {
    let files = collect_files(work, keep)?;
    let mut before = Vec::with_capacity(files.len());
    for rel in &files {
        let abs = work.join(rel);
        before.push(std::fs::read_to_string(&abs).map_err(|s| io_err(&abs, s))?);
    }

    fix(work, &files)?;

    let mut subtracted = Vec::new();
    for (rel, old) in files.iter().zip(before) {
        let abs = work.join(rel);
        let new = std::fs::read_to_string(&abs).map_err(|s| io_err(&abs, s))?;
        if new != old {
            subtracted.push(SubtractedFile {
                rel_path: rel.clone(),
                new_content: new,
            });
        }
    }
    Ok(subtracted)
}

/// Test-only handle on the private engine tail, so an adapter's unit tests can
/// exercise the full produce→finalize path without making [`finalize_changes`] or
/// [`SubtractedFile`]'s construction part of any public surface.
#[cfg(test)]
pub(crate) fn finalize_changes_for_test(
    subtracted: Vec<SubtractedFile>,
    work: &Path,
    repo: &Path,
) -> Result<Vec<PlannedChange>, MigrateError> {
    finalize_changes(subtracted, work, repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn finalize_emits_overwrite_for_a_changed_file() {
        let repo = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        fs::write(repo.path().join("a.py"), "import os\nx = 1\n").unwrap();
        fs::write(work.path().join("a.py"), "import os\nx = 1\n").unwrap();

        let subtracted = vec![SubtractedFile {
            rel_path: PathBuf::from("a.py"),
            new_content: "x = 1\n".to_string(),
        }];
        let changes = finalize_changes(subtracted, work.path(), repo.path()).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, ChangeAction::Overwrite);
        assert_eq!(changes[0].path, repo.path().join("a.py"));
        assert_eq!(changes[0].new_content, "x = 1\n");
        assert_eq!(
            changes[0].old_content.as_deref(),
            Some("import os\nx = 1\n")
        );
    }

    #[test]
    fn finalize_drops_a_no_op_rewrite() {
        let repo = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        fs::write(repo.path().join("a.py"), "x = 1\n").unwrap();
        fs::write(work.path().join("a.py"), "x = 1\n").unwrap();

        let subtracted = vec![SubtractedFile {
            rel_path: PathBuf::from("a.py"),
            new_content: "x = 1\n".to_string(),
        }];
        assert!(finalize_changes(subtracted, work.path(), repo.path())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn finalize_rejects_an_absolute_path_from_a_buggy_adapter() {
        let repo = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("evil.py"), "secret\n").unwrap();

        let subtracted = vec![SubtractedFile {
            rel_path: outside.path().join("evil.py"),
            new_content: String::new(),
        }];
        assert!(finalize_changes(subtracted, work.path(), repo.path())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn finalize_rejects_a_dotdot_escape() {
        let repo = TempDir::new().unwrap();
        let work = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("evil.py"), "secret\n").unwrap();

        // A `..` traversal out of the isolated tree must not resolve to a real file.
        let escape = Path::new("..").join(outside.path().file_name().unwrap());
        let subtracted = vec![SubtractedFile {
            rel_path: escape.join("evil.py"),
            new_content: String::new(),
        }];
        assert!(finalize_changes(subtracted, work.path(), repo.path())
            .unwrap()
            .is_empty());
    }
}
