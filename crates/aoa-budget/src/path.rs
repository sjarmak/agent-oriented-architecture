use std::path::{Path, PathBuf};

/// Lexically normalize `.` and `..` components without touching the filesystem.
///
/// Context closure resolution and linting both compare operator-authored local
/// references through this exact lexical rule.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        use std::path::Component::*;
        match component {
            CurDir => {}
            ParentDir => match out.components().next_back() {
                Some(Normal(_)) => {
                    out.pop();
                }
                Some(RootDir | Prefix(_)) => {}
                Some(ParentDir | CurDir) | None => out.push(component.as_os_str()),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_lexically_without_filesystem_access() {
        assert_eq!(
            normalize_path(Path::new("docs/./guide/../rules.md")),
            PathBuf::from("docs/rules.md")
        );
    }

    #[test]
    fn keeps_a_parent_step_that_leaves_the_starting_directory() {
        assert_eq!(
            normalize_path(Path::new("../AGENTS.md")),
            PathBuf::from("../AGENTS.md")
        );
        assert_eq!(
            normalize_path(Path::new("pkg/../../shared/./rules.md")),
            PathBuf::from("../shared/rules.md")
        );
        assert_eq!(
            normalize_path(Path::new("../../a/../b.md")),
            PathBuf::from("../../b.md")
        );
    }

    #[test]
    fn a_parent_step_at_the_filesystem_root_stays_at_the_root() {
        assert_eq!(
            normalize_path(Path::new("/../etc/rules.md")),
            PathBuf::from("/etc/rules.md")
        );
    }
}
