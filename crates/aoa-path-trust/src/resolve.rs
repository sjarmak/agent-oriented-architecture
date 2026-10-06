//! Resolving a caller-supplied path that need not exist yet.
//!
//! The third shape of the trust question, and the reason it is spelled
//! separately from [`super::nofollow`]. That walker answers "does this path
//! contain a link right now?" and refuses one outright. A write hook instead
//! names a file that may not exist, under directories that legitimately may be
//! links, and needs the *destination* the write would land on so the caller can
//! check containment itself. So this resolves symlinks rather than rejecting
//! them, and leaves the containment verdict to the caller.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use super::PathTrustError;

/// Reduce `.` and `..` textually, without touching the filesystem.
///
/// Keeps the caller's spelling of a path that may be reached through a link, so
/// a policy can match the alias as well as the destination.
pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(".."),
            },
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

/// Resolve `candidate` to the location a write would actually reach, even when
/// its trailing components do not exist yet.
///
/// [`Path::canonicalize`] cannot be applied at the leaf, because the leaf may be
/// missing. Existing components are canonicalized as they are encountered
/// (which resolves symlinks); after the first missing component, `.` and `..`
/// are reduced lexically until an existing ancestor is reached again.
///
/// Containment is *not* checked here: the caller owns the trust root and must
/// verify the returned path still lies beneath it.
pub fn resolve_canonicalizing(candidate: &Path) -> Result<PathBuf, PathTrustError> {
    resolve_written_path(candidate).map(|written| written.canonical)
}

pub const MAX_LINKS_FOLLOWED: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPath {
    pub canonical: PathBuf,
    pub spellings: Vec<LinkSpelling>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpelling {
    pub parent: PathBuf,
    pub spelled: PathBuf,
}

enum Step {
    Root(OsString),
    Up,
    Name(OsString),
}

fn steps(path: &Path) -> impl Iterator<Item = Step> + '_ {
    path.components().filter_map(|component| match component {
        Component::Prefix(_) | Component::RootDir => {
            Some(Step::Root(component.as_os_str().to_os_string()))
        }
        Component::CurDir => None,
        Component::ParentDir => Some(Step::Up),
        Component::Normal(name) => Some(Step::Name(name.to_os_string())),
    })
}

fn unresolved(rest: &VecDeque<Step>) -> PathBuf {
    rest.iter().fold(PathBuf::new(), |mut path, step| {
        match step {
            Step::Root(root) => path.push(root),
            Step::Up => path.push(".."),
            Step::Name(name) => path.push(name),
        }
        path
    })
}

pub fn resolve_written_path(candidate: &Path) -> Result<WrittenPath, PathTrustError> {
    let mut pending: VecDeque<Step> = steps(candidate).collect();
    let mut resolved = if candidate.has_root() {
        PathBuf::new()
    } else {
        let cwd =
            std::env::current_dir().map_err(|source| PathTrustError::io(candidate, source))?;
        cwd.canonicalize()
            .map_err(|source| PathTrustError::io(cwd, source))?
    };
    let mut missing = 0usize;
    let mut followed = 0usize;
    let mut spellings = Vec::new();

    while let Some(step) = pending.pop_front() {
        match step {
            Step::Root(root) => {
                resolved = PathBuf::from(root);
                missing = 0;
            }
            Step::Up => {
                resolved.pop();
                missing = missing.saturating_sub(1);
            }
            Step::Name(name) if missing > 0 => {
                resolved.push(name);
                missing += 1;
            }
            Step::Name(name) => {
                let next = resolved.join(&name);
                let metadata = match std::fs::symlink_metadata(&next) {
                    Ok(metadata) => metadata,
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                        resolved = next;
                        missing = 1;
                        continue;
                    }
                    Err(source) => return Err(PathTrustError::io(next, source)),
                };
                if !metadata.is_symlink() {
                    resolved = next;
                    continue;
                }
                if followed == MAX_LINKS_FOLLOWED {
                    return Err(PathTrustError::TooManyLinks {
                        path: next,
                        limit: MAX_LINKS_FOLLOWED,
                    });
                }
                followed += 1;
                let target = std::fs::read_link(&next)
                    .map_err(|source| PathTrustError::io(&next, source))?;
                spellings.push(LinkSpelling {
                    parent: resolved.clone(),
                    spelled: next.join(unresolved(&pending)),
                });
                for step in steps(&target).collect::<Vec<_>>().into_iter().rev() {
                    pending.push_front(step);
                }
            }
        }
    }
    Ok(WrittenPath {
        canonical: resolved,
        spellings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_reduction_drops_cur_dir_and_pops_parents() {
        assert_eq!(
            normalize_lexically(Path::new("/repo/./a/b/../c")),
            PathBuf::from("/repo/a/c")
        );
        assert_eq!(
            normalize_lexically(Path::new("a/../../b/../../c")),
            PathBuf::from("../../c")
        );
    }

    #[test]
    fn a_parent_at_the_filesystem_root_stays_at_the_root_lexically() {
        for spelled in ["/..", "/../..", "/../../etc", "/a/../../etc"] {
            let expected = if spelled.ends_with("etc") {
                PathBuf::from("/etc")
            } else {
                PathBuf::from("/")
            };
            assert_eq!(
                normalize_lexically(Path::new(spelled)),
                expected,
                "{spelled}"
            );
        }
    }

    #[test]
    fn a_parent_at_the_filesystem_root_stays_at_the_root_when_resolving() {
        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        let below_root = PathBuf::from("/..").join(base.strip_prefix("/").expect("absolute"));

        assert_eq!(
            resolve_canonicalizing(Path::new("/..")).expect("resolve"),
            PathBuf::from("/")
        );
        assert_eq!(
            resolve_canonicalizing(Path::new("/../..")).expect("resolve"),
            PathBuf::from("/")
        );
        assert_eq!(
            resolve_written_path(&below_root.join("leaf.rs")).expect("resolve"),
            WrittenPath {
                canonical: base.join("leaf.rs"),
                spellings: Vec::new(),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_target_beginning_with_a_parent_at_the_root_resolves_from_the_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        std::fs::create_dir(base.join("real")).expect("create real");
        let below_root = PathBuf::from("/..").join(base.strip_prefix("/").expect("absolute"));
        symlink(below_root.join("real"), base.join("alias")).expect("plant alias");

        let written = resolve_written_path(&base.join("alias/leaf.rs")).expect("resolve");

        assert_eq!(
            written,
            WrittenPath {
                canonical: base.join("real/leaf.rs"),
                spellings: vec![LinkSpelling {
                    parent: base.clone(),
                    spelled: base.join("alias/leaf.rs"),
                }],
            }
        );
    }

    #[test]
    fn missing_leaf_components_resolve_under_their_existing_ancestor() {
        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");

        assert_eq!(
            resolve_canonicalizing(&base.join("absent/leaf.rs")).expect("resolve"),
            base.join("absent/leaf.rs")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_symlink_resolves_to_its_destination() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        let outside = tempfile::tempdir().expect("create outside");
        let outside_base = outside.path().canonicalize().expect("canonical outside");
        symlink(&outside_base, base.join("linked")).expect("plant link");

        assert_eq!(
            resolve_canonicalizing(&base.join("linked/leaf.rs")).expect("resolve"),
            outside_base.join("leaf.rs"),
            "the resolver reports the real destination so callers can refuse it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn every_link_passed_records_the_spelling_from_its_canonical_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        std::fs::create_dir_all(base.join("a")).expect("create a");
        std::fs::create_dir_all(base.join("b")).expect("create b");
        std::fs::create_dir_all(base.join("c")).expect("create c");
        symlink(base.join("b/l2"), base.join("a/l1")).expect("plant l1");
        symlink("../c", base.join("b/l2")).expect("plant l2");

        let written = resolve_written_path(&base.join("a/l1/sub/leaf.rs")).expect("resolve");

        assert_eq!(written.canonical, base.join("c/sub/leaf.rs"));
        assert_eq!(
            written.spellings,
            vec![
                LinkSpelling {
                    parent: base.join("a"),
                    spelled: base.join("a/l1/sub/leaf.rs"),
                },
                LinkSpelling {
                    parent: base.join("b"),
                    spelled: base.join("b/l2/sub/leaf.rs"),
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_loop_is_refused_at_the_bound_instead_of_followed() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        symlink("second", base.join("first")).expect("plant first");
        symlink("first", base.join("second")).expect("plant second");

        let refused = resolve_written_path(&base.join("first/leaf.rs")).unwrap_err();

        assert!(
            matches!(
                &refused,
                PathTrustError::TooManyLinks { path, limit }
                    if *limit == MAX_LINKS_FOLLOWED && (path == &base.join("first") || path == &base.join("second"))
            ),
            "{refused}"
        );
        assert!(matches!(
            resolve_canonicalizing(&base.join("first/leaf.rs")),
            Err(PathTrustError::TooManyLinks { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_chain_as_long_as_the_bound_still_resolves() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        std::fs::create_dir(base.join("real")).expect("create real");
        symlink("real", base.join("link0")).expect("plant link0");
        for hop in 1..MAX_LINKS_FOLLOWED {
            symlink(format!("link{}", hop - 1), base.join(format!("link{hop}")))
                .expect("plant link");
        }
        let last = base.join(format!("link{}", MAX_LINKS_FOLLOWED - 1));

        let written = resolve_written_path(&last.join("leaf.rs")).expect("resolve");

        assert_eq!(written.canonical, base.join("real/leaf.rs"));
        assert_eq!(written.spellings.len(), MAX_LINKS_FOLLOWED);
        symlink(&last, base.join("one_more")).expect("plant one more");
        assert!(matches!(
            resolve_written_path(&base.join("one_more/leaf.rs")),
            Err(PathTrustError::TooManyLinks { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn parents_re_enter_the_existing_tree_after_a_missing_component() {
        let root = tempfile::tempdir().expect("create root");
        let base = root.path().canonicalize().expect("canonical root");
        std::fs::create_dir(base.join("real")).expect("create dir");

        assert_eq!(
            resolve_canonicalizing(&base.join("absent/../real/leaf.rs")).expect("resolve"),
            base.join("real/leaf.rs")
        );
    }
}
