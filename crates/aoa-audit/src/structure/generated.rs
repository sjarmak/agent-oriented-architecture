use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use super::declarations::LINGUIST_GENERATED_ATTR;
use super::io_err;
use crate::error::AuditError;

const ATTRIBUTES_FILE: &str = ".gitattributes";
const MAX_ATTRIBUTES_BYTES: u64 = 256 * 1024;
const MAX_GENERATED_RULES: usize = 2_000;
const MACRO_PREFIX: &str = "[attr]";

struct Rule {
    matcher: GlobMatcher,
    generated: bool,
}

pub(super) struct GeneratedMarks {
    repo: PathBuf,
    rules_by_dir: BTreeMap<PathBuf, Vec<Rule>>,
    rules_loaded: usize,
}

impl GeneratedMarks {
    pub(super) fn new(repo: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
            rules_by_dir: BTreeMap::new(),
            rules_loaded: 0,
        }
    }

    pub(super) fn is_generated(&mut self, path: &Path) -> Result<bool, AuditError> {
        let dirs = path
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.repo));
        for dir in dirs {
            if !self.rules_by_dir.contains_key(dir) {
                let rules = read_rules(dir, MAX_GENERATED_RULES - self.rules_loaded)?;
                self.rules_loaded += rules.len();
                self.rules_by_dir.insert(dir.to_path_buf(), rules);
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

fn read_rules(dir: &Path, remaining: usize) -> Result<Vec<Rule>, AuditError> {
    use std::io::Read as _;
    let path = dir.join(ATTRIBUTES_FILE);
    let is_regular_file = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file());
    if !is_regular_file {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(&path).map_err(|source| io_err(&path, source))?;
    let mut raw = Vec::new();
    file.take(MAX_ATTRIBUTES_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|source| io_err(&path, source))?;
    if raw.len() as u64 > MAX_ATTRIBUTES_BYTES {
        return Err(AuditError::GeneratedAttributesOverLimit {
            path,
            limit: "256 KiB in one file",
        });
    }
    let rules: Vec<Rule> = String::from_utf8_lossy(&raw)
        .lines()
        .filter_map(parse_rule)
        .take(remaining + 1)
        .collect();
    if rules.len() > remaining {
        return Err(AuditError::GeneratedAttributesOverLimit {
            path,
            limit: "2000 linguist-generated rules across the repository",
        });
    }
    Ok(rules)
}

fn parse_rule(line: &str) -> Option<Rule> {
    let mut tokens = line.split_whitespace();
    let pattern = tokens.next()?;
    if pattern.starts_with('#') || pattern.starts_with(MACRO_PREFIX) || pattern.ends_with('/') {
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

    #[test]
    fn a_character_class_pattern_is_a_pattern_and_a_macro_line_is_not() {
        let dir = tempfile::tempdir().unwrap();
        attributes(
            dir.path(),
            "[attr]made linguist-generated\n[ab]*.rs linguist-generated\n",
        );
        assert!(generated(dir.path(), "alpha.rs"));
        assert!(!generated(dir.path(), "made"));
        assert!(!generated(dir.path(), "core.rs"));
    }

    #[test]
    fn more_rules_than_the_repository_limit_fail_the_measure_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let half: String = (0..=MAX_GENERATED_RULES / 2)
            .map(|n| format!("p{n}* linguist-generated\n"))
            .collect();
        attributes(dir.path(), &half);
        attributes(&dir.path().join("pkg"), &half);

        let err = GeneratedMarks::new(dir.path())
            .is_generated(&dir.path().join("pkg/lib.rs"))
            .unwrap_err();

        assert!(
            matches!(&err, AuditError::GeneratedAttributesOverLimit { path, .. } if path.ends_with(ATTRIBUTES_FILE)),
            "{err}"
        );
    }

    #[test]
    fn an_oversized_attributes_file_fails_the_measure_instead_of_reading_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let padding = "# pad\n".repeat(MAX_ATTRIBUTES_BYTES as usize / 6 + 1);
        attributes(dir.path(), &format!("{padding}*.rs linguist-generated\n"));

        let err = GeneratedMarks::new(dir.path())
            .is_generated(&dir.path().join("lib.rs"))
            .unwrap_err();

        assert!(
            matches!(err, AuditError::GeneratedAttributesOverLimit { .. }),
            "{err}"
        );
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
