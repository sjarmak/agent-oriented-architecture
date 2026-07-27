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
            ParentDir => {
                out.pop();
            }
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
}
