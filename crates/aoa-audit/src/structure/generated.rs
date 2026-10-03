use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use super::read_source_capped;
use crate::error::AuditError;

const ATTRIBUTES_FILE: &str = ".gitattributes";
const LINGUIST_GENERATED_ATTR: &str = "linguist-generated";

struct Rule {
    matcher: GlobMatcher,
    generated: bool,
}

pub(super) struct GeneratedMarks {
    repo: PathBuf,
    rules_by_dir: BTreeMap<PathBuf, Vec<Rule>>,
}

impl GeneratedMarks {
    pub(super) fn new(repo: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
            rules_by_dir: BTreeMap::new(),
        }
    }

    pub(super) fn is_generated(&mut self, path: &Path) -> Result<bool, AuditError> {
        let dirs = path
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.repo));
        for dir in dirs {
            if !self.rules_by_dir.contains_key(dir) {
                self.rules_by_dir
                    .insert(dir.to_path_buf(), read_rules(dir)?);
            }
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };
            let decided = self.rules_by_dir[dir]
                .iter()
                .rev()
                .find(|rule| rule.matcher.is_match(relative))
                .map(|rule| rule.generated);
            if let Some(generated) = decided {
                return Ok(generated);
            }
        }
        Ok(false)
    }
}

fn read_rules(dir: &Path) -> Result<Vec<Rule>, AuditError> {
    let path = dir.join(ATTRIBUTES_FILE);
    let is_regular_file = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
    if !is_regular_file {
        return Ok(Vec::new());
    }
    let Some(text) = read_source_capped(&path)? else {
        return Ok(Vec::new());
    };
    Ok(text.lines().filter_map(parse_rule).collect())
}

fn parse_rule(line: &str) -> Option<Rule> {
    let mut tokens = line.split_whitespace();
    let pattern = tokens.next()?;
    if pattern.starts_with('#') || pattern.starts_with('[') || pattern.ends_with('/') {
        return None;
    }
    let generated = tokens.filter_map(generated_state).next_back()?;
    let glob = if pattern.contains('/') {
        pattern.trim_start_matches('/').to_string()
    } else {
        format!("**/{pattern}")
    };
    let matcher = GlobBuilder::new(&glob)
        .literal_separator(true)
        .build()
        .ok()?
        .compile_matcher();
    Some(Rule { matcher, generated })
}

fn generated_state(attribute: &str) -> Option<bool> {
    if let Some(unset) = attribute
        .strip_prefix('-')
        .or_else(|| attribute.strip_prefix('!'))
    {
        return (unset == LINGUIST_GENERATED_ATTR).then_some(false);
    }
    match attribute.split_once('=') {
        Some((LINGUIST_GENERATED_ATTR, value)) => Some(value != "false"),
        None if attribute == LINGUIST_GENERATED_ATTR => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn generated(root: &Path, relative: &str) -> bool {
        GeneratedMarks::new(root)
            .is_generated(&root.join(relative))
            .unwrap()
    }

    fn attributes(dir: &Path, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(ATTRIBUTES_FILE), text).unwrap();
    }

    #[test]
    fn a_repo_without_attributes_marks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!generated(dir.path(), "src/api.rs"));
    }

    #[test]
    fn a_basename_pattern_matches_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        attributes(dir.path(), "*.pb.go linguist-generated\n");
        assert!(generated(dir.path(), "api.pb.go"));
        assert!(generated(dir.path(), "svc/deep/api.pb.go"));
        assert!(!generated(dir.path(), "svc/api.go"));
    }

    #[test]
    fn a_pattern_with_a_slash_is_anchored_to_its_attributes_file() {
        let dir = tempfile::tempdir().unwrap();
        attributes(
            dir.path(),
            "src/gen/** linguist-generated=true\n/schema/*.ts linguist-generated\n/top.rs linguist-generated\n",
        );
        assert!(generated(dir.path(), "src/gen/a/b.rs"));
        assert!(!generated(dir.path(), "other/src/gen/b.rs"));
        assert!(generated(dir.path(), "schema/types.ts"));
        assert!(!generated(dir.path(), "schema/nested/types.ts"));
        assert!(generated(dir.path(), "top.rs"));
        assert!(!generated(dir.path(), "nested/top.rs"));
    }

    #[test]
    fn the_last_matching_line_wins_and_can_unset_the_mark() {
        let dir = tempfile::tempdir().unwrap();
        attributes(
            dir.path(),
            "gen/** linguist-generated\n\
             gen/kept.rs -linguist-generated\n\
             gen/also.rs linguist-generated=false\n\
             gen/unspecified.rs !linguist-generated\n",
        );
        assert!(generated(dir.path(), "gen/made.rs"));
        assert!(!generated(dir.path(), "gen/kept.rs"));
        assert!(!generated(dir.path(), "gen/also.rs"));
        assert!(!generated(dir.path(), "gen/unspecified.rs"));
    }

    #[test]
    fn lines_that_carry_no_generated_attribute_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        attributes(
            dir.path(),
            "# *.rs linguist-generated\n*.rs text eol=lf\n*.rs linguist-vendored\n",
        );
        assert!(!generated(dir.path(), "lib.rs"));
    }

    #[test]
    fn a_nested_attributes_file_overrides_its_parent_within_its_subtree() {
        let dir = tempfile::tempdir().unwrap();
        attributes(dir.path(), "*.rs linguist-generated\n");
        attributes(&dir.path().join("pkg"), "hand.rs -linguist-generated\n");
        assert!(!generated(dir.path(), "pkg/hand.rs"));
        assert!(generated(dir.path(), "pkg/made.rs"));
        assert!(generated(dir.path(), "other/hand.rs"));
    }

    #[test]
    fn attributes_above_the_repo_root_are_not_consulted() {
        let outer = tempfile::tempdir().unwrap();
        attributes(outer.path(), "*.rs linguist-generated\n");
        let repo = outer.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        assert!(!generated(&repo, "lib.rs"));
    }

    #[test]
    fn a_non_utf8_attributes_file_is_read_lossily() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(ATTRIBUTES_FILE),
            [b"\xff\n".as_slice(), b"*.rs linguist-generated\n"].concat(),
        )
        .unwrap();
        assert!(generated(dir.path(), "lib.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_attributes_file_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("attrs");
        fs::write(&target, "*.rs linguist-generated\n").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(ATTRIBUTES_FILE)).unwrap();
        assert!(!generated(dir.path(), "lib.rs"));
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "*.rs linguist-generated\n"
        );
    }
}
