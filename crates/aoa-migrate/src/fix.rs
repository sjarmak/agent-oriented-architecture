//! The [`CodeFix`] trait and the migrations that implement it.
//!
//! A [`CodeFix`] is a *planner*: given a checkout it returns the
//! [`PlannedChange`]s it would make, reading the tree but writing nothing.
//! Applying the plan is the separate, reversible step in [`crate::apply`].

use std::path::{Path, PathBuf};

use crate::error::MigrateError;

/// Whether a [`PlannedChange`] creates a new file or overwrites an existing one.
///
/// The two shapes roll back differently: a [`Create`](ChangeAction::Create) is
/// undone by deleting the file, an [`Overwrite`](ChangeAction::Overwrite) by
/// restoring the archived original. Modeling this as data (not a runtime guess
/// at apply time) is what lets rollback be exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeAction {
    /// The target file does not exist; the migration will create it.
    Create,
    /// The target file exists; the migration will archive then replace it.
    Overwrite,
}

/// A single file change a [`CodeFix`] proposes. Produced by planning (which
/// writes nothing) and consumed by both the dry-run diff and the apply step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChange {
    /// Absolute path of the file to write.
    pub path: PathBuf,
    /// Whether this creates or overwrites.
    pub action: ChangeAction,
    /// The content that will be written.
    pub new_content: String,
    /// The current content, when overwriting (drives the diff preview). `None`
    /// for a create.
    pub old_content: Option<String>,
}

/// A mechanical, reproducible, oracle-blind migration toward a code-layer
/// best-practice. Implementors only *plan*; the engine applies.
pub trait CodeFix {
    /// Stable machine identifier, recorded in the migration manifest.
    fn id(&self) -> &str;

    /// One-line human description of what the fix does.
    fn describe(&self) -> &str;

    /// Compute the changes this fix would make to `repo`. Read-only: a `plan`
    /// call never mutates the checkout.
    fn plan(&self, repo: &Path) -> Result<Vec<PlannedChange>, MigrateError>;
}

/// Directory names skipped when listing a package's contents — build output and
/// vendored trees are not part of the navigable structure. Mirrors the audit's
/// own walk so the anchor describes the same tree the audit measured.
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "vendor",
    "dist",
    "build",
    "__pycache__",
];

/// The filename written for the navigability anchor.
const ANCHOR_FILENAME: &str = "README.md";

/// Creates a navigability anchor (a `README.md`) at every package root that the
/// audit reports as lacking one. The anchor's content is a **pure function of
/// the directory tree** — the package name is its directory name and the body
/// is a sorted index of immediate entries. It reads no file *bodies*, so it
/// cannot transcribe a held-out task answer into the most-read file in the
/// package (the guardrail-2 leak channel), and identical trees always produce
/// byte-identical anchors (reproducible by construction).
///
/// Construct-validity note: this fix exists to satisfy a pre-registered
/// best-practice — "every package root carries a navigability anchor" — that
/// the audit independently *verifies*. The audit's count dropping to zero is a
/// consequence of meeting that spec, not the definition of success (anti-
/// Goodhart; see `docs/r0_runbook.md` guardrail 3).
pub struct NavigabilityAnchorFix;

impl CodeFix for NavigabilityAnchorFix {
    fn id(&self) -> &str {
        "navigability-anchor"
    }

    fn describe(&self) -> &str {
        "create a mechanical README index at each package root lacking a navigability anchor"
    }

    fn plan(&self, repo: &Path) -> Result<Vec<PlannedChange>, MigrateError> {
        let mut sites = aoa_audit::navigability_sites(repo)?;
        // Deterministic plan order regardless of directory-read order, so the
        // diff preview and manifest are reproducible.
        sites.sort();

        sites
            .iter()
            .map(|site| {
                Ok(PlannedChange {
                    path: site.join(ANCHOR_FILENAME),
                    action: ChangeAction::Create,
                    new_content: anchor_content(site)?,
                    old_content: None,
                })
            })
            .collect()
    }
}

/// Build the navigability-anchor body for `dir` from tree structure alone.
///
/// Title is the directory's own name; the body is a sorted index of immediate
/// entries (directories suffixed with `/`), skipping hidden and build-output
/// names. No file contents, timestamps, or absolute paths enter the output, so
/// the result is deterministic and leak-free.
fn anchor_content(dir: &Path) -> Result<String, MigrateError> {
    let title = dir
        .file_name()
        .map(|n| sanitize(&n.to_string_lossy()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| ".".to_string());

    let mut entries: Vec<String> = Vec::new();
    for entry in read_dir(dir)? {
        let entry = entry.map_err(|source| io_err(dir, source))?;
        let raw = entry.file_name();
        let raw = raw.to_string_lossy();
        if raw.starts_with('.') || SKIP_DIRS.contains(&raw.as_ref()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        // `file_type` does not follow symlinks; a symlink is listed by its name
        // without descending, matching the audit's never-follow-symlinks walk.
        let name = sanitize(&raw);
        if file_type.is_dir() {
            entries.push(format!("{name}/"));
        } else {
            entries.push(name);
        }
    }
    entries.sort();

    let mut body = format!(
        "# {title}\n\n\
         <!-- Navigability anchor generated by `aoa migrate`. Mechanical index \
         of this package's top-level entries; regenerate with `aoa migrate \
         --apply`. -->\n\n\
         ## Contents\n\n"
    );
    if entries.is_empty() {
        body.push_str("_(no indexable entries)_\n");
    } else {
        for name in &entries {
            body.push_str(&format!("- `{name}`\n"));
        }
    }
    Ok(body)
}

/// Neutralize a filename for safe embedding in the markdown anchor: control
/// characters (newlines, CR, tab) that would break list/heading structure
/// become `_`, and backticks that would close an inline code span are dropped.
/// Hostile names require write access to the repo (which already permits direct
/// edits), so this is defense-in-depth against cosmetic corruption of the
/// generated anchor, not a trust boundary.
fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '`')
        .map(|c| if c.is_control() { '_' } else { c })
        .collect()
}

fn read_dir(dir: &Path) -> Result<std::fs::ReadDir, MigrateError> {
    std::fs::read_dir(dir).map_err(|source| io_err(dir, source))
}

fn io_err(path: &Path, source: std::io::Error) -> MigrateError {
    MigrateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aoa-migrate-fix-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn plans_an_anchor_for_each_readme_less_root() {
        let dir = tmp("plan-sites");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let pkg = dir.join("crate-a");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("Cargo.toml"), "[package]\n").unwrap();

        let changes = NavigabilityAnchorFix.plan(&dir).unwrap();
        let targets: Vec<&PathBuf> = changes.iter().map(|c| &c.path).collect();
        assert!(targets.contains(&&dir.join("README.md")));
        assert!(targets.contains(&&pkg.join("README.md")));
        assert!(changes.iter().all(|c| c.action == ChangeAction::Create));
        assert!(changes.iter().all(|c| c.old_content.is_none()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plans_nothing_when_every_root_has_a_readme() {
        let dir = tmp("plan-none");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        assert!(NavigabilityAnchorFix.plan(&dir).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_is_read_only() {
        let dir = tmp("plan-readonly");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let before: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        NavigabilityAnchorFix.plan(&dir).unwrap();
        let after: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(before.len(), after.len(), "plan must not create files");
        assert!(!dir.join("README.md").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_content_lists_sorted_entries_and_marks_dirs() {
        let dir = tmp("content-sorted");
        fs::write(dir.join("zeta.rs"), "z\n").unwrap();
        fs::write(dir.join("alpha.rs"), "a\n").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        // Hidden and build dirs are excluded from the index.
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();

        let body = anchor_content(&dir).unwrap();
        let alpha = body.find("alpha.rs").unwrap();
        let src = body.find("src/").unwrap();
        let zeta = body.find("zeta.rs").unwrap();
        assert!(alpha < src && src < zeta, "entries must be sorted");
        assert!(body.contains("- `src/`"), "directories suffixed with /");
        assert!(!body.contains(".git"), "hidden dirs excluded");
        assert!(!body.contains("target"), "build dirs excluded");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_content_is_blind_to_file_bodies() {
        // The leak-channel invariant: identical trees whose files differ only in
        // content must produce byte-identical anchors. If the generator ever
        // read a file body, this would fail — and a held-out answer could leak.
        let a = tmp("blind-a");
        let b = tmp("blind-b");
        for d in [&a, &b] {
            fs::write(d.join("lib.rs"), "").unwrap();
            fs::create_dir_all(d.join("src")).unwrap();
        }
        fs::write(a.join("lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
        fs::write(b.join("lib.rs"), "// totally different body\n").unwrap();

        // Compare with the directory name normalized away (the title is the dir
        // name, which legitimately differs); the index body must be identical.
        let strip_title = |s: String| s.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(
            strip_title(anchor_content(&a).unwrap()),
            strip_title(anchor_content(&b).unwrap()),
            "anchor must depend on tree structure, not file contents"
        );
        fs::remove_dir_all(&a).ok();
        fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn anchor_content_neutralizes_hostile_filenames() {
        let dir = tmp("content-hostile");
        // A filename with a backtick (would close the inline code span) and one
        // with control bytes (would break list/heading structure).
        fs::write(dir.join("we`ird.rs"), "x\n").unwrap();
        fs::write(dir.join("tab\tname.rs"), "x\n").unwrap();

        let body = anchor_content(&dir).unwrap();
        assert!(!body.contains("we`ird"), "backtick stripped from entry");
        assert!(!body.contains('\t'), "control char neutralized");
        // Every list line is a single, well-formed entry.
        for line in body.lines().filter(|l| l.starts_with("- ")) {
            assert_eq!(line.matches('`').count(), 2, "balanced code span: {line}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_content_is_deterministic_across_runs() {
        let dir = tmp("content-deterministic");
        fs::write(dir.join("a.rs"), "a\n").unwrap();
        fs::write(dir.join("b.rs"), "b\n").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();

        assert_eq!(anchor_content(&dir).unwrap(), anchor_content(&dir).unwrap());
        fs::remove_dir_all(&dir).ok();
    }
}
