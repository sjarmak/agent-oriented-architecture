use std::path::{Path, PathBuf};
use std::process::Command;

use hegel::{generators as gs, TestCase};

mod common;

use common::workspace_root;

const HOME_ROOTS: [&str; 2] = [concat!("/", "home/"), concat!("/", "Users/")];

fn contains_operator_home(line: &str) -> bool {
    HOME_ROOTS.iter().any(|root| {
        line.match_indices(root).any(|(offset, _)| {
            line[offset + root.len()..]
                .split_once('/')
                .is_some_and(|(name, _)| !name.is_empty())
        })
    })
}

fn tracked_paths(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .expect("git ls-files runs");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            PathBuf::from(
                String::from_utf8(path.to_vec()).expect("tracked file paths are valid UTF-8"),
            )
        })
        .collect()
}

fn text_contents(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn violations(root: &Path) -> Vec<String> {
    tracked_paths(root)
        .into_iter()
        .filter_map(|relative| {
            text_contents(&root.join(&relative)).map(|contents| (relative, contents))
        })
        .flat_map(|(relative, contents)| {
            contents
                .lines()
                .enumerate()
                .filter(|(_, line)| contains_operator_home(line))
                .map(move |(index, _)| format!("{}:{}", relative.display(), index + 1))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn tracked_text_files_do_not_disclose_operator_home_paths() {
    let found = violations(&workspace_root());
    assert!(
        found.is_empty(),
        "tracked text files disclose operator home paths:\n{}",
        found.join("\n")
    );
}

#[test]
fn path_detection_requires_a_user_and_trailing_separator() {
    assert!(contains_operator_home(concat!("/", "home/alice/project")));
    assert!(contains_operator_home(concat!("/", "Users/alice/project")));
    assert!(!contains_operator_home(concat!("/", "home//project")));
    assert!(!contains_operator_home(concat!("/", "home/alice")));
    assert!(!contains_operator_home("home/alice/project"));
}

#[hegel::test]
fn every_embedded_operator_home_path_is_detected(tc: TestCase) {
    let root = tc.draw(gs::sampled_from(HOME_ROOTS.to_vec()));
    let surrounding = tc.draw(gs::text());
    let input = format!("{surrounding}{root}operator/{surrounding}");
    assert!(contains_operator_home(&input));
}
