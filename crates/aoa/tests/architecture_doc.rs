//! CLAUDE.md's crate list is a contract, not a comment (aoa-1uen5).
//!
//! The "Architecture Overview" section assigns every library crate to exactly
//! one layer. That assignment is what a contributor reads to decide where new
//! code goes, so a crate missing from the list has no declared layer at all and
//! a listed crate that no longer exists points at nothing. Both drift silently:
//! adding a crate to `crates/` does not touch the document, and neither
//! `cargo build` nor clippy reads prose.
//!
//! `aoa-falsify-build` is the case that motivated this test — it was split out
//! of `aoa-falsify`, never added to the list, and was the only crate in the
//! workspace with no declared layer.
//!
//! Scope is deliberately membership, not edge direction. A layer-order check
//! (no crate depending on a later layer) is worth having, but it surfaces two
//! pre-existing inversions that belong to their own beads, so it is tracked
//! separately rather than bolted on here where it would have to ship with an
//! exception list longer than the rule.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The prose immediately above the layer bullets. The list is anchored to it
/// rather than to a heading so that reordering sections cannot silently point
/// the parser at a different bullet block. Kept short enough to sit on one
/// line: the surrounding sentence is hard-wrapped, so a longer anchor would
/// span a newline and never match.
const LIST_ANCHOR: &str = "remaining crates are narrow libraries:";

/// The composition root, named in the anchor prose rather than in the bullets
/// ("the *remaining* crates"). It is the one crate legitimately absent from the
/// layer list.
const CLI_CRATE: &str = "aoa";

fn workspace_root() -> PathBuf {
    // crates/aoa -> crates -> workspace root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/aoa sits two levels below the workspace root")
        .to_path_buf()
}

/// The crate names CLAUDE.md's layer bullets assign a layer to, in document
/// order and including any duplicate so the caller can report it.
///
/// Panics rather than returning an empty set when the anchor or the bullets are
/// missing: a parser that quietly finds nothing would turn every assertion below
/// into a vacuous pass, which is worse than having no test.
fn documented_crates(claude_md: &str) -> Vec<String> {
    let (_, after_anchor) = claude_md
        .split_once(LIST_ANCHOR)
        .unwrap_or_else(|| panic!("CLAUDE.md no longer contains the anchor {LIST_ANCHOR:?}"));

    let mut names = Vec::new();
    let mut saw_bullet = false;
    for line in after_anchor.lines() {
        if line.trim().is_empty() {
            // One blank line separates the anchor from the block; the next one
            // ends it. Stopping here is what keeps the prose below the bullets
            // (which also names crates in backticks) out of the layer list.
            if saw_bullet {
                break;
            }
            continue;
        }
        let is_bullet = line.starts_with("- ");
        // Bullets wrap onto continuation lines that begin with whitespace.
        let is_continuation = saw_bullet && line.starts_with("  ");
        if !is_bullet && !is_continuation {
            break;
        }
        saw_bullet = true;
        names.extend(backticked_crate_names(line));
    }

    assert!(
        !names.is_empty(),
        "CLAUDE.md's layer bullets named no crates; the list format changed"
    );
    names
}

/// Every `` `aoa-…` `` token on one line. The list writes each crate name in
/// backticks, so this reads the names without depending on how the bullets wrap
/// or how the layers are worded.
fn backticked_crate_names(line: &str) -> Vec<String> {
    line.split('`')
        // Odd-indexed pieces are the ones between a pair of backticks.
        .skip(1)
        .step_by(2)
        .filter(|token| token.starts_with("aoa"))
        .map(str::to_string)
        .collect()
}

/// The layer list as CLAUDE.md currently states it.
fn layer_list() -> Vec<String> {
    let claude_md =
        std::fs::read_to_string(workspace_root().join("CLAUDE.md")).expect("CLAUDE.md is readable");
    documented_crates(&claude_md)
}

/// Every directory under `crates/` that is a real crate (carries a Cargo.toml).
fn workspace_crates(root: &Path) -> BTreeSet<String> {
    let crates_dir = root.join("crates");
    let entries = std::fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", crates_dir.display()));

    let mut names = BTreeSet::new();
    for entry in entries {
        let entry = entry.expect("reading a crates/ entry");
        if !entry.path().join("Cargo.toml").is_file() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .expect("crate directory name is UTF-8");
        names.insert(name);
    }

    assert!(
        !names.is_empty(),
        "found no crates under {}",
        crates_dir.display()
    );
    names
}

#[test]
fn every_library_crate_has_exactly_one_documented_layer() {
    let documented = layer_list();
    let existing = workspace_crates(&workspace_root());

    let expected: BTreeSet<&str> = existing
        .iter()
        .map(String::as_str)
        .filter(|name| *name != CLI_CRATE)
        .collect();
    let listed: BTreeSet<&str> = documented.iter().map(String::as_str).collect();

    let undocumented: Vec<&&str> = expected.difference(&listed).collect();
    assert!(
        undocumented.is_empty(),
        "these crates exist but no CLAUDE.md layer claims them: {undocumented:?} — \
a crate with no declared layer is where the next contributor puts code in the wrong place"
    );

    let phantom: Vec<&&str> = listed.difference(&expected).collect();
    assert!(
        phantom.is_empty(),
        "CLAUDE.md assigns a layer to crates that do not exist under crates/: {phantom:?}"
    );
}

#[test]
fn no_crate_is_listed_under_two_layers() {
    let documented = layer_list();

    let mut seen = BTreeSet::new();
    let duplicates: Vec<&String> = documented
        .iter()
        .filter(|name| !seen.insert(name.as_str()))
        .collect();
    assert!(
        duplicates.is_empty(),
        "these crates appear in more than one layer bullet: {duplicates:?} — \
a crate has one layer or the list decides nothing"
    );
}

#[test]
fn the_cli_crate_is_not_given_a_library_layer() {
    let documented = layer_list();

    assert!(
        !documented.iter().any(|name| name == CLI_CRATE),
        "the bullets describe `the remaining crates` — the {CLI_CRATE} composition root \
must stay out of them"
    );
}
