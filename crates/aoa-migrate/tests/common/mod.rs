//! Shared helpers for the dead-import integration suites. Each `tests/*.rs` file
//! is its own binary, so this module is included via `mod common;` rather than
//! imported as a crate.

use std::fs;
use std::path::Path;

/// Sorted list of (relative path, bytes-len) for every regular file under `root`,
/// excluding the throwaway `target/` a build would create. Used to assert the real
/// repo is structurally unchanged by a dry-run plan.
pub fn tree_snapshot(root: &Path) -> Vec<(String, u64)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, u64)>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                if entry.file_name() == "target" {
                    continue;
                }
                walk(&path, base, out);
            } else if ft.is_file() {
                let rel = path.strip_prefix(base).unwrap().display().to_string();
                out.push((rel, entry.metadata().unwrap().len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}
