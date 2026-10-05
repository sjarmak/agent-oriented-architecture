//! Resolving a caller-supplied path that need not exist yet.
//!
//! The third shape of the trust question, and the reason it is spelled
//! separately from [`super::nofollow`]. That walker answers "does this path
//! contain a link right now?" and refuses one outright. A write hook instead
//! names a file that may not exist, under directories that legitimately may be
//! links, and needs the *destination* the write would land on so the caller can
//! check containment itself. So this resolves symlinks rather than rejecting
//! them, and leaves the containment verdict to the caller.

use std::path::{Component, Path, PathBuf};

use super::PathTrustError;

/// Reduce `.` and `..` textually, without touching the filesystem.
///
/// Keeps the caller's spelling of a path that may be reached through a link, so
/// a policy can match the alias as well as the destination.
pub fn normalize_lexically(path: &Path) -> Result<PathBuf, PathTrustError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(PathTrustError::EscapesRoot {
                        path: path.to_path_buf(),
                    });
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
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
    let mut resolved = PathBuf::new();
    let mut missing_component = false;

    for component in candidate.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err(PathTrustError::EscapesRoot {
                        path: candidate.to_path_buf(),
                    });
                }
                if missing_component {
                    if let Some(canonical) = canonicalize_existing_component(&resolved)? {
                        resolved = canonical;
                        missing_component = false;
                    }
                }
            }
            Component::Normal(part) => {
                resolved.push(part);
                if !missing_component {
                    if let Some(canonical) = canonicalize_existing_component(&resolved)? {
                        resolved = canonical;
                    } else {
                        missing_component = true;
                    }
                }
            }
        }
    }
    Ok(resolved)
}

/// Canonicalize `path` when it exists. `None` means "does not exist yet"; every
/// other failure is surfaced rather than being read as absent.
fn canonicalize_existing_component(path: &Path) -> Result<Option<PathBuf>, PathTrustError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => path
            .canonicalize()
            .map(Some)
            .map_err(|source| PathTrustError::io(path, source)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PathTrustError::io(path, source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_reduction_drops_cur_dir_and_pops_parents() {
        assert_eq!(
            normalize_lexically(Path::new("/repo/./a/b/../c")).expect("reduce"),
            PathBuf::from("/repo/a/c")
        );
        assert!(matches!(
            normalize_lexically(Path::new("/..")),
            Err(PathTrustError::EscapesRoot { .. })
        ));
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
