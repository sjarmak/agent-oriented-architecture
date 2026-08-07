//! What a documentation test needs before it can read the document (aoa-g7yxv).
//!
//! Three integration-test targets in this directory assert that a file under
//! the workspace root still says what the workspace does: `architecture_doc.rs`
//! (CLAUDE.md's crate list), `decision_records.rs` (`docs/adr/`), and
//! `environment_dependent_skips.rs` (ADR 0004 and the sites it classifies).
//! Each one starts from the same two steps — find the workspace root from
//! `CARGO_MANIFEST_DIR`, then read a path relative to it and panic loudly if it
//! is missing — and each had grown its own byte-identical copy of both.
//!
//! Loud is the load-bearing part. A test that reads a document and quietly gets
//! nothing back passes while checking nothing, which is the failure these
//! targets exist to prevent; so absence is a panic here rather than a `Result`
//! the caller could shrug off.

use std::path::{Path, PathBuf};

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
