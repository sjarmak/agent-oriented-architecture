use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use aoa_path_trust::{
    normalize_lexically, resolve_canonicalizing, resolve_repository_root, resolve_written_path,
    RepositoryRootError,
};

#[cfg(test)]
use aoa_path_trust::git_free_of_inherited_state;

use super::HookEvent;

pub(super) struct GovernedWrite {
    pub(super) roots: Vec<PathBuf>,
    pub(super) spellings: Vec<PathBuf>,
}

pub(super) fn governed_write(base: &Path, candidate: Option<&Path>) -> Result<GovernedWrite> {
    let mut roots = vec![base.to_path_buf()];
    while let Some(root) = enclosing_repository(roots.last().and_then(|root| root.parent()))? {
        roots.push(root);
    }
    let (Some(candidate), Some(outermost)) = (candidate, roots.last()) else {
        return Ok(GovernedWrite {
            roots,
            spellings: Vec::new(),
        });
    };
    if scope_under(outermost, candidate)? == WriteScope::Outside {
        return Ok(GovernedWrite {
            roots: Vec::new(),
            spellings: Vec::new(),
        });
    }

    let written = resolve_written_path(candidate)?;
    let mut starts = vec![existing_directory_above(&written.canonical)?];
    starts.extend(
        written
            .spellings
            .iter()
            .map(|spelling| Some(spelling.parent.clone())),
    );
    for start in starts {
        let mut enclosing = enclosing_repository(start.as_deref())?;
        while let Some(root) = enclosing.filter(|root| !roots.contains(root)) {
            enclosing = enclosing_repository(root.parent())?;
            roots.push(root);
        }
    }
    roots.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });

    let mut spellings = vec![candidate.to_path_buf()];
    for spelling in written.spellings {
        if !spellings.contains(&spelling.spelled) {
            spellings.push(spelling.spelled);
        }
    }
    Ok(GovernedWrite { roots, spellings })
}

pub(super) fn scope_of_spellings(root: &Path, spellings: &[PathBuf]) -> Result<WriteScope> {
    let mut targets = Vec::new();
    for spelling in spellings {
        if let WriteScope::Inside(found) = scope_under(root, spelling)? {
            for target in found {
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
        }
    }
    Ok(if targets.is_empty() {
        WriteScope::Outside
    } else {
        WriteScope::Inside(targets)
    })
}

fn enclosing_repository(directory: Option<&Path>) -> Result<Option<PathBuf>> {
    match directory.map(resolve_repository_root) {
        None | Some(Err(RepositoryRootError::NotInRepository { .. })) => Ok(None),
        Some(Ok(root)) => Ok(Some(root)),
        Some(Err(refusal)) => Err(refusal.into()),
    }
}

fn existing_directory_above(canonical: &Path) -> Result<Option<PathBuf>> {
    for ancestor in canonical.ancestors().skip(1) {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => return Ok(Some(ancestor.to_path_buf())),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(anyhow!(err))
                    .with_context(|| format!("failed to inspect {}", ancestor.display()))
            }
        }
    }
    Ok(None)
}

pub(super) fn write_target(event: &HookEvent) -> Option<&str> {
    event
        .tool_input
        .get("file_path")
        .or_else(|| event.tool_input.get("notebook_path"))
        .and_then(Value::as_str)
}

/// Whether a pending write lands inside the enforcing repository, and if so
/// under which repository-relative spellings the path policies must match it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WriteScope {
    /// Inside: every repository-relative spelling relevant to path policy — the
    /// lexical hook spelling and the symlink-resolved destination. Matching
    /// both prevents an in-repository symlink from hiding either a protected
    /// alias or a protected destination.
    Inside(Vec<String>),
    /// Outside: neither spelling lands in the repository, so no policy this
    /// repository declares has anything to say about the write.
    Outside,
}

pub(super) fn write_candidate(event: &HookEvent) -> Result<Option<PathBuf>> {
    write_target(event)
        .map(|raw| anchor_target(raw, &event.cwd))
        .transpose()
}

fn anchor_target(raw: &str, cwd: &str) -> Result<PathBuf> {
    if raw.is_empty() {
        return Err(anyhow!("hook write target must not be empty"));
    }
    let target = Path::new(raw);
    if target.is_absolute() {
        return Ok(target.to_path_buf());
    }
    if cwd.is_empty() {
        return Err(anyhow!(
            "hook cwd is absent, so relative write target {raw:?} cannot be anchored"
        ));
    }
    let cwd = Path::new(cwd);
    if !cwd.is_absolute() {
        return Err(anyhow!(
            "hook cwd must be absolute to anchor relative write target {raw:?}: {cwd:?}"
        ));
    }
    Ok(cwd.join(target))
}

pub(super) fn scope_under(root: &Path, candidate: &Path) -> Result<WriteScope> {
    let lexical = contained(root, &normalize_lexically(candidate))?;
    let resolved = contained(root, &resolve_canonicalizing(candidate)?)?;

    Ok(match (lexical, resolved) {
        (None, None) => WriteScope::Outside,
        (Some(lexical), Some(resolved)) if lexical == resolved => {
            WriteScope::Inside(vec![resolved])
        }
        (Some(lexical), Some(resolved)) => WriteScope::Inside(vec![lexical, resolved]),
        (Some(only), None) | (None, Some(only)) => WriteScope::Inside(vec![only]),
    })
}

/// The repository-relative spelling of one resolution of a hook target, or
/// `None` when that resolution lands outside the repository.
fn contained(base: &Path, path: &Path) -> Result<Option<String>> {
    let Ok(relative) = path.strip_prefix(base) else {
        return Ok(None);
    };
    if relative.as_os_str().is_empty() {
        return Err(anyhow!("hook write target resolves to repository root"));
    }
    relative
        .to_str()
        .map(|relative| Some(relative.to_owned()))
        .ok_or_else(|| anyhow!("resolved hook write target is not UTF-8: {relative:?}"))
}

#[cfg(test)]
pub(super) fn init_git_repo(path: &Path) {
    let status = git_free_of_inherited_state()
        .args(["init", "--quiet"])
        .arg(path)
        .status()
        .expect("git is available for repository-boundary tests");
    assert!(status.success(), "git init failed for test fixture");
}

#[cfg(test)]
pub(super) fn repositories_enclosing(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut next = enclosing_repository(Some(directory)).unwrap();
    while let Some(root) = next {
        next = enclosing_repository(root.parent()).unwrap();
        found.push(root);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(base: &Path, raw: &str) -> WriteScope {
        let candidate = anchor_target(raw, base.to_str().unwrap()).expect("anchored hook target");
        scope_under(base, &candidate).expect("supported hook target")
    }

    #[test]
    fn in_repository_shapes_are_in_scope_under_one_normalized_spelling() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let absolute = base.join(".github/workflows/ci.yml");

        for raw in [
            absolute.to_str().unwrap(),
            "./.github/workflows/ci.yml",
            "src/../.github/workflows/ci.yml",
        ] {
            assert_eq!(
                scope(&base, raw),
                WriteScope::Inside(vec![".github/workflows/ci.yml".to_string()]),
                "{raw} resolves back inside the repository"
            );
        }
    }

    /// The bead's subject (aoa-7g14y.1): a target in another directory tree is
    /// not this repository's business, however it is spelled.
    #[test]
    fn targets_outside_the_repository_are_out_of_scope() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let absolute = elsewhere.path().join("notes.md");

        assert_eq!(scope(&base, "../outside.rs"), WriteScope::Outside);
        assert_eq!(
            scope(&base, absolute.to_str().unwrap()),
            WriteScope::Outside
        );
        // A `..` chain walking off the filesystem root cannot be inside either.
        assert_eq!(
            scope(&base, "../../../../../../../../../../etc/passwd"),
            WriteScope::Outside
        );
    }

    /// A repository-local symlink pointing out of the tree stays in scope under
    /// its lexical spelling, so the R5 protected-path match still sees it.
    #[cfg(unix)]
    #[test]
    fn a_repo_local_symlink_leaving_the_tree_stays_in_scope() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), base.join("escape")).unwrap();

        assert_eq!(
            scope(&base, "escape/planted.rs"),
            WriteScope::Inside(vec!["escape/planted.rs".to_string()])
        );
    }

    #[cfg(unix)]
    #[test]
    fn governed_write_lists_a_root_every_chain_reaches_once_and_a_spelling_once() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let above = repositories_enclosing(&root);
        let outer = root.join("outer");
        let nested = outer.join("nested");
        init_git_repo(&outer);
        init_git_repo(&nested);
        std::fs::create_dir_all(nested.join("real/src")).unwrap();
        std::os::unix::fs::symlink(nested.join("real"), nested.join("alias")).unwrap();
        let candidate = nested.join("alias/src/lib.rs");

        let governed = governed_write(&outer, Some(&candidate)).unwrap();

        assert_eq!(
            governed.roots,
            [vec![nested.clone(), outer.clone()], above].concat()
        );
        assert_eq!(governed.spellings, vec![candidate]);
    }

    #[cfg(unix)]
    #[test]
    fn governed_write_orders_roots_innermost_first_whichever_chain_found_them() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let above = repositories_enclosing(&root);
        let outer = root.join("outer");
        let q = outer.join("q");
        init_git_repo(&outer);
        init_git_repo(&q);
        std::fs::create_dir_all(outer.join("real")).unwrap();
        std::os::unix::fs::symlink(outer.join("real"), q.join("linked")).unwrap();

        let governed = governed_write(&outer, Some(&q.join("linked/src.rs"))).unwrap();

        assert_eq!(
            governed.roots,
            [vec![q.clone(), outer.clone()], above].concat()
        );
        assert_eq!(
            governed.spellings,
            vec![q.join("linked/src.rs")],
            "the link's spelling is the written path itself"
        );
    }

    #[cfg(unix)]
    #[test]
    fn governed_write_collects_every_repository_a_chain_of_links_passes_through() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let above = repositories_enclosing(&root);
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        init_git_repo(&a);
        init_git_repo(&b);
        init_git_repo(&c);
        std::os::unix::fs::symlink(b.join("l2"), a.join("l1")).unwrap();
        std::os::unix::fs::symlink(&c, b.join("l2")).unwrap();

        let governed = governed_write(&a, Some(&a.join("l1/src.rs"))).unwrap();

        assert_eq!(
            governed.roots,
            [vec![a.clone(), b.clone(), c.clone()], above].concat()
        );
        assert_eq!(
            governed.spellings,
            vec![a.join("l1/src.rs"), b.join("l2/src.rs")]
        );
    }

    /// Containment is the only thing that stopped being an error. A target that
    /// names nothing writable is still a failure, and `check` denies on it.
    #[test]
    fn unusable_write_targets_are_still_errors_not_out_of_scope() {
        let repo = tempfile::tempdir().unwrap();
        let base = repo.path().canonicalize().unwrap();

        let cwd = base.to_str().unwrap();
        assert!(anchor_target("", cwd).is_err());
        assert!(scope_under(&base, &anchor_target(".", cwd).unwrap())
            .unwrap_err()
            .to_string()
            .contains("repository root"));
    }

    #[test]
    fn a_relative_target_is_anchored_at_the_hook_cwd_and_an_absolute_one_stands_alone() {
        assert_eq!(
            anchor_target("../src.rs", "/O/sub").unwrap(),
            PathBuf::from("/O/sub/../src.rs")
        );
        assert_eq!(
            anchor_target("/elsewhere/src.rs", "/O/sub").unwrap(),
            PathBuf::from("/elsewhere/src.rs")
        );
        assert_eq!(
            anchor_target("/elsewhere/src.rs", "").unwrap(),
            PathBuf::from("/elsewhere/src.rs")
        );
    }

    #[test]
    fn a_relative_target_without_an_absolute_hook_cwd_is_refused() {
        let absent = anchor_target("src.rs", "").unwrap_err().to_string();
        assert!(
            absent.contains("hook cwd is absent") && absent.contains("src.rs"),
            "{absent}"
        );

        let relative = anchor_target("src.rs", "O/sub").unwrap_err().to_string();
        assert!(
            relative.contains("must be absolute") && relative.contains("O/sub"),
            "{relative}"
        );
    }
}
