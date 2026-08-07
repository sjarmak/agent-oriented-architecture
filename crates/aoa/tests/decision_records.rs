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
//!
//! A third way is the record number itself (aoa-2qhuj). Two branches can each
//! add a record under the same next-free number and both land: the index gets a
//! row for each, both files exist, and every check above still passes while a
//! citation of "ADR 0005" no longer names one record. Numbers are how these
//! records are cited, so a duplicated one is a broken citation.

use std::collections::{BTreeMap, BTreeSet};

mod common;

use common::{read, workspace_root};

/// The index every citation resolves to, relative to the workspace root.
const INDEX: &str = "docs/adr/README.md";

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

/// The leading number a record is cited by, or `None` when the name does not
/// start with one.
///
/// A record with no number is its own defect: it cannot be cited, and it would
/// slip past the uniqueness check below by having nothing to compare.
fn record_number(name: &str) -> Option<&str> {
    let (number, _) = name.split_once('-')?;
    let is_number = number.len() >= 4 && number.chars().all(|c| c.is_ascii_digit());
    is_number.then_some(number)
}

/// Record names grouped by the number they are cited by, keeping only the
/// numbers that more than one record claims.
fn numbers_claimed_more_than_once(names: &BTreeSet<String>) -> BTreeMap<String, Vec<String>> {
    let mut by_number: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in names {
        let Some(number) = record_number(name) else {
            continue;
        };
        by_number
            .entry(number.to_owned())
            .or_default()
            .push(name.clone());
    }
    by_number.retain(|_, claimants| claimants.len() > 1);
    by_number
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

#[test]
fn every_decision_record_is_numbered() {
    let unnumbered: Vec<String> = adr_files()
        .into_iter()
        .filter(|name| record_number(name).is_none())
        .collect();

    assert!(
        unnumbered.is_empty(),
        "docs/adr/ holds records that do not start with a four-digit number, so \
         there is no way to cite them and nothing for the uniqueness check to \
         compare: {unnumbered:?}"
    );
}

#[test]
fn no_two_decision_records_share_a_number() {
    let collisions = numbers_claimed_more_than_once(&adr_files());

    assert!(
        collisions.is_empty(),
        "docs/adr/ holds records that share a number, so citing that number no \
         longer names one record; renumber the later one and fix its index row \
         and every reference to its path: {collisions:?}"
    );
}

/// The collision this file exists to catch, on synthetic names: without this,
/// `no_two_decision_records_share_a_number` would pass just as happily against
/// a detector that never reports anything.
#[test]
fn two_records_sharing_a_number_are_reported() {
    let names: BTreeSet<String> = [
        "0004-environment-dependent-test-skips.md",
        "0005-enforcement-liveness-in-a-checkout.md",
        "0005-architecture-model-conformance.md",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();

    let collisions = numbers_claimed_more_than_once(&names);

    assert_eq!(
        collisions.keys().collect::<Vec<_>>(),
        vec!["0005"],
        "the two records numbered 0005 are the collision; 0004 is not"
    );
    assert_eq!(collisions["0005"].len(), 2);
}
