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
//! Scope is membership *and* edge direction. The order check landed with
//! aoa-ynqcn, which split `aoa-domain` out of `aoa-gap` and removed the first of
//! the two inversions this file used to describe — `aoa-bench` no longer reaches
//! into measurement to name a held-out subject. The second, `aoa-recommend` into
//! controlled changes, is gone with aoa-4s25v: `recommend` now takes the
//! migration registry as owned rows the composition root projects for it. So
//! `LAYER_EXCEPTIONS` is empty, which is the state it is meant to be in — an
//! entry is a licence for a tracked edge to stay wrong, not a place to park one.

use std::collections::{BTreeMap, BTreeSet};
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

/// The crate names each layer bullet claims, one inner `Vec` per bullet, in
/// document order. Bullet order *is* layer order: the list runs bottom-up, so a
/// crate may depend on its own bullet or an earlier one and never a later one.
///
/// Panics rather than returning an empty list when the anchor or the bullets are
/// missing: a parser that quietly finds nothing would turn every assertion below
/// into a vacuous pass, which is worse than having no test.
fn layer_bullets() -> Vec<Vec<String>> {
    let claude_md =
        std::fs::read_to_string(workspace_root().join("CLAUDE.md")).expect("CLAUDE.md is readable");
    let (_, after_anchor) = claude_md
        .split_once(LIST_ANCHOR)
        .unwrap_or_else(|| panic!("CLAUDE.md no longer contains the anchor {LIST_ANCHOR:?}"));

    let mut bullets: Vec<Vec<String>> = Vec::new();
    for line in after_anchor.lines() {
        if line.trim().is_empty() {
            // One blank line separates the anchor from the block; the next one
            // ends it. Stopping here is what keeps the prose below the bullets
            // (which also names crates in backticks) out of the layer list.
            if !bullets.is_empty() {
                break;
            }
            continue;
        }
        let is_bullet = line.starts_with("- ");
        // Bullets wrap onto continuation lines that begin with whitespace.
        let is_continuation = !bullets.is_empty() && line.starts_with("  ");
        if !is_bullet && !is_continuation {
            break;
        }
        if is_bullet {
            bullets.push(Vec::new());
        }
        let names = backticked_crate_names(line);
        bullets.last_mut().expect("a bullet is open").extend(names);
    }

    assert!(
        bullets.iter().any(|bullet| !bullet.is_empty()),
        "CLAUDE.md's layer bullets named no crates; the list format changed"
    );
    bullets
}

/// The crate names CLAUDE.md's layer bullets assign a layer to, in document
/// order and including any duplicate so the caller can report it.
fn layer_list() -> Vec<String> {
    layer_bullets().into_iter().flatten().collect()
}

/// Which bullet claims each crate, by index. Duplicates are reported by
/// [`no_crate_is_listed_under_two_layers`]; here the first wins so that a
/// duplicate produces one clear failure rather than two confusing ones.
fn layer_index() -> BTreeMap<String, usize> {
    let mut index = BTreeMap::new();
    for (depth, bullet) in layer_bullets().into_iter().enumerate() {
        for name in bullet {
            index.entry(name).or_insert(depth);
        }
    }
    index
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

/// Every library crate on disk: a directory under `crates/` carrying a
/// Cargo.toml, minus the CLI composition root, which the bullets exclude by
/// construction.
fn library_crates() -> BTreeSet<String> {
    let crates_dir = workspace_root().join("crates");
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
        if name != CLI_CRATE {
            names.insert(name);
        }
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
    let on_disk = library_crates();
    let listed: BTreeSet<String> = layer_list().into_iter().collect();

    let undocumented: Vec<&String> = on_disk.difference(&listed).collect();
    assert!(
        undocumented.is_empty(),
        "these crates exist but no CLAUDE.md layer claims them: {undocumented:?} — \
a crate with no declared layer is where the next contributor puts code in the wrong place"
    );

    let phantom: Vec<&String> = listed.difference(&on_disk).collect();
    assert!(
        phantom.is_empty(),
        "CLAUDE.md gives a layer to {phantom:?}, which is not a library crate under crates/"
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

/// The internal (`aoa-*`, path-dependency) crates one manifest names under
/// `[dependencies]`.
///
/// `[dev-dependencies]` is deliberately excluded. A dev-dependency does not
/// enter any consumer's build graph, so it cannot invert the layering for
/// anyone downstream — `aoa-bench` keeps `aoa-gap` there precisely so its
/// loader-to-gate integration test can run without the crate carrying a
/// production edge into measurement.
fn declared_dependencies(crate_name: &str) -> BTreeSet<String> {
    let manifest_path = workspace_root()
        .join("crates")
        .join(crate_name)
        .join("Cargo.toml");
    let manifest: toml::Table = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()))
        .parse()
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));

    manifest
        .get("dependencies")
        .and_then(|deps| deps.as_table())
        .into_iter()
        .flat_map(|deps| deps.keys())
        .filter(|name| name.starts_with("aoa-"))
        .cloned()
        .collect()
}

/// Edges that point at a later layer and are known, tracked, and not this
/// file's to fix. An entry is a licence to stay wrong while a bead is open, so
/// each one names that bead — and [`layer_exceptions_are_all_still_live`]
/// deletes the licence the moment the edge goes away.
const LAYER_EXCEPTIONS: &[(&str, &str, &str)] = &[];

#[test]
fn no_crate_depends_on_a_later_layer() {
    let depth = layer_index();

    let mut inversions = Vec::new();
    for (crate_name, crate_depth) in &depth {
        for dep in declared_dependencies(crate_name) {
            let Some(dep_depth) = depth.get(&dep) else {
                // A dependency with no layer is already the subject of
                // `every_library_crate_has_exactly_one_documented_layer`;
                // reporting it twice would just bury that clearer failure.
                continue;
            };
            if dep_depth > crate_depth
                && !LAYER_EXCEPTIONS
                    .iter()
                    .any(|(from, to, _)| from == crate_name && *to == dep)
            {
                inversions.push(format!("{crate_name} -> {dep}"));
            }
        }
    }

    assert!(
        inversions.is_empty(),
        "these dependencies point at a later CLAUDE.md layer: {inversions:?} — \
the documented direction is domain -> inputs -> measurement -> decisions -> \
controlled changes and enforcement -> CLI, so either the edge is wrong or the \
layer assignment is. Add to LAYER_EXCEPTIONS only with a bead id and a reason"
    );
}

#[test]
fn layer_exceptions_are_all_still_live() {
    let depth = layer_index();

    let stale: Vec<&str> = LAYER_EXCEPTIONS
        .iter()
        .filter(|(from, to, _)| {
            let inverted = match (depth.get(*from), depth.get(*to)) {
                (Some(from_depth), Some(to_depth)) => to_depth > from_depth,
                // Either crate having no layer means the exception no longer
                // describes anything the order check would look at.
                _ => false,
            };
            !(inverted && declared_dependencies(from).contains(*to))
        })
        .map(|(_, _, reason)| *reason)
        .collect();

    assert!(
        stale.is_empty(),
        "these LAYER_EXCEPTIONS entries no longer describe a real inversion: {stale:?} — \
delete them. An exception list nobody prunes stops being a record of known debt and \
becomes a place to hide new debt"
    );
}
