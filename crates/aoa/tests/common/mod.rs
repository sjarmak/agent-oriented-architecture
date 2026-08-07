//! What a documentation test needs before it can read the document, and the
//! facts about the workspace's crates that more than one of them has to agree
//! on (aoa-g7yxv, aoa-enzj8).
//!
//! Several integration-test targets in this directory assert that a file under
//! the workspace root still says what the workspace does, and each had grown
//! its own byte-identical copy of the two steps that takes: find the workspace
//! root from `CARGO_MANIFEST_DIR`, then read a path relative to it and panic
//! loudly if it is missing.
//!
//! Loud is the load-bearing part. A test that reads a document and quietly gets
//! nothing back passes while checking nothing, which is the failure these
//! targets exist to prevent; so absence is a panic here rather than a `Result`
//! the caller could shrug off.
//!
//! Two of those targets go further and read the crates themselves.
//! `architecture_doc.rs` holds CLAUDE.md's layer list to the crates on disk;
//! `architecture_model.rs` holds `architecture/model.c4` to the same crates.
//! Both have to answer "which crates are there" and "what does this one really
//! depend on", and the second answer is the load-bearing one: a *production*
//! edge is what can invert a layer or falsify an arrow, and a dev-dependency is
//! neither. Two copies of that definition is where the next drift lives, so
//! there is one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The CLI composition root. It is a crate like any other, but it is not a
/// library: CLAUDE.md's bullets exclude it by construction ("the *remaining*
/// crates"), and `model.c4` draws it as a container rather than a component.
pub const CLI_CRATE: &str = "aoa";

/// The workspace root, derived from this crate's manifest directory.
pub fn workspace_root() -> PathBuf {
    // crates/aoa -> crates -> workspace root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/aoa sits two levels below the workspace root")
        .to_path_buf()
}

/// Read a workspace-relative file, panicking with the path when it is not
/// readable.
pub fn read(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|e| panic!("{relative} is readable: {e}"))
}

/// Every library crate on disk: a directory under `crates/` carrying a
/// Cargo.toml, minus the CLI composition root.
///
/// Panics rather than returning an empty set: a silently empty listing would
/// turn every membership assertion into a vacuous pass.
pub fn library_crates() -> BTreeSet<String> {
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

/// The internal (`aoa-*`, path-dependency) crates one manifest names as a
/// production dependency, across the plain `[dependencies]` table and every
/// `[target.'cfg(…)'.dependencies]` table.
///
/// `[dev-dependencies]` is deliberately excluded. A dev-dependency does not
/// enter any consumer's build graph, so it cannot invert the layering for
/// anyone downstream — `aoa-recommend` keeps `aoa-migrate` there precisely so
/// its drift guard can read the real migration registry without restoring the
/// production edge aoa-4s25v removed.
///
/// The target-specific tables hold no `aoa-*` crate today. They are read anyway
/// because a cfg-gated internal dependency is a production edge on the
/// platforms it applies to, and a checker that read only the plain table would
/// report green while the build graph said otherwise.
pub fn declared_dependencies(crate_name: &str) -> BTreeSet<String> {
    let manifest_path = workspace_root()
        .join("crates")
        .join(crate_name)
        .join("Cargo.toml");
    let manifest: toml::Table = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()))
        .parse()
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));

    let plain = manifest.get("dependencies").into_iter();
    let per_target = manifest
        .get("target")
        .and_then(|targets| targets.as_table())
        .into_iter()
        .flat_map(|targets| targets.values())
        .filter_map(|target| target.get("dependencies"));

    plain
        .chain(per_target)
        .filter_map(|deps| deps.as_table())
        .flat_map(|deps| deps.keys())
        .filter(|name| name.starts_with("aoa-"))
        .cloned()
        .collect()
}
