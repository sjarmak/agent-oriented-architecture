//! `docs/adr/` is a cited path, not a suggestion (aoa-kv8j9).
//!
//! The standing reinvention gate asks every new work item to cite a scan of the
//! decision-record index before proposing work. That citation is only worth
//! anything if the path resolves: before this test, `docs/adr/` did not exist at
//! all, so the required scan was of nothing and every citation was vacuous.
//!
//! Two ways that decays again, both silent to `cargo build`:
//! CLAUDE.md can name a path that no longer exists, and an ADR can be added to
//! the directory without ever reaching the index a reader actually opens. This
//! file fails on both.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The index every citation resolves to, relative to the workspace root.
const INDEX: &str = "docs/adr/README.md";

fn workspace_root() -> PathBuf {
    // crates/aoa -> crates -> workspace root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/aoa sits two levels below the workspace root")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{relative} is readable: {e}"))
}

/// The ADR files present on disk, excluding the index itself.
///
/// Panics rather than returning an empty set when the directory is missing: a
/// silently empty listing would make every assertion below a vacuous pass,
/// which is the exact failure mode this test exists to catch.
fn adr_files() -> BTreeSet<String> {
    let dir = workspace_root().join("docs/adr");
    let entries = std::fs::read_dir(&dir).expect("docs/adr/ exists and is readable");

    let mut names = BTreeSet::new();
    for entry in entries {
        let name = entry.expect("docs/adr/ entry is readable").file_name();
        let name = name.to_string_lossy().into_owned();
        if name == "README.md" || !name.ends_with(".md") {
            continue;
        }
        names.insert(name);
    }
    names
}

#[test]
fn the_decision_record_index_exists() {
    let index = read(INDEX);
    assert!(
        !index.trim().is_empty(),
        "{INDEX} exists but is empty, so a citation of it still resolves to nothing"
    );
}

#[test]
fn claude_md_points_at_the_index() {
    let claude_md = read("CLAUDE.md");
    assert!(
        claude_md.contains(INDEX),
        "CLAUDE.md no longer names {INDEX}, so the reinvention gate's citation \
         requirement has no path in this repository to resolve against"
    );
}

#[test]
fn every_decision_record_is_listed_in_the_index() {
    let index = read(INDEX);
    let unlisted: Vec<String> = adr_files()
        .into_iter()
        .filter(|name| !index.contains(name.as_str()))
        .collect();

    assert!(
        unlisted.is_empty(),
        "docs/adr/ holds records the index never links, so a reader who scans \
         {INDEX} would not see them: {unlisted:?}"
    );
}

#[test]
fn the_index_links_only_records_that_exist() {
    let index = read(INDEX);
    let present = adr_files();
    let missing: Vec<String> = index
        .split(['(', ')'])
        .filter(|token| token.ends_with(".md") && !token.contains('/') && *token != "README.md")
        .map(str::to_owned)
        .filter(|link| !present.contains(link))
        .collect();

    assert!(
        missing.is_empty(),
        "{INDEX} links records that do not exist in docs/adr/: {missing:?}"
    );
}
