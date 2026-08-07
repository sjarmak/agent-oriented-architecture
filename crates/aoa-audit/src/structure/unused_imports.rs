//! The unused-import proxy probe and the `use`-statement scanner it needs.
//!
//! Split out of the probe registry because parsing Rust syntax is a different
//! reason to change from measuring a repo: this is the only probe carrying its
//! own language front-end, and the registry should not grow one every time a
//! probe wants to look inside a file. Everything below `unused_import_measure`
//! and [`unused_import_proxy_item`] — the two entry points [`super::PROBES`]
//! names — is that front-end.
//!
//! The scanner is a declared approximation. `aoa-migrate` gets a
//! rustc-certified answer from `cargo check --message-format=json`; the audit
//! runs against repos it must not build, so it approximates from tokens and
//! says so in the probe's name.

use std::path::{Path, PathBuf};

use aoa_metrics::SubtreePartition;

use super::{
    collect_source_line_counts, common_subtree, io_err, is_rust_file, read_dir, read_source_capped,
    StructureMeasure, MAX_WALK_DEPTH, SKIP_DIRS,
};
use crate::error::AuditError;
use crate::punch::{FindingKind, MeasuredCost, PunchItem};
use crate::tier::Tier;

/// The unused-import proxy count, or [`StructureMeasure::Unmeasurable`] on a repo
/// with no Rust sources — the proxy is Rust-only, so its absence is "no data", not
/// "zero unused imports". A repo that has Rust sources with every import used is a
/// real `Measured(0)`.
pub(super) fn unused_import_measure(repo: &Path) -> Result<StructureMeasure, AuditError> {
    let mut sources: Vec<(PathBuf, u64)> = Vec::new();
    collect_source_line_counts(repo, &mut sources, 0)?;
    let has_rust = sources
        .iter()
        .any(|(p, _)| p.extension().and_then(|e| e.to_str()) == Some("rs"));
    if !has_rust {
        return Ok(StructureMeasure::Unmeasurable);
    }
    let mut unused: Vec<(PathBuf, u64)> = Vec::new();
    collect_unused_imports(repo, &mut unused, 0)?;
    Ok(StructureMeasure::Measured(
        unused.iter().map(|(_, n)| n).sum(),
    ))
}

/// Count likely-unused imports across the Rust sources under `repo`, by a cheap
/// SYNTACTIC proxy: per file, a `use`-bound name that never appears as an
/// identifier token in the file body is *likely* unused.
///
/// This is a measured fact about syntax, not a compiler verdict — it shells out
/// to nothing and writes nothing, preserving the audit's zero-write contract. It
/// is deliberately INDEPENDENT of any migration that removes unused imports: the
/// compiler *defines* the exact unused set; this proxy only *observes* the
/// direction, so an `aoa-migrate` dead-import fix is verified against a number it
/// did not produce (anti-Goodhart; the R0 verify-not-define discipline).
///
/// The proxy is lossy by contract, and biased toward UNDER-counting: any textual
/// mention of a name — even in a comment or string — marks it used, and `pub use`
/// re-exports (never compiler-unused) are excluded outright. It still over-counts
/// a few classes a syntactic scan cannot resolve without type information: trait
/// imports used only through method calls (`use std::io::Read` then `r.read(..)`),
/// names reachable only through a glob, macro-expanded uses, and `cfg`-gated code.
/// Those false positives are exactly why the measure is born [`Tier::Tier3`] and
/// cannot gate until external-outcome correlation promotes it.
///
/// Non-Rust repos (no `.rs` files) and clean repos (zero likely-unused imports)
/// both produce no finding — the punch-list reports only positive measured facts,
/// mirroring the sibling structure checks. R0's repo-delta arm reads the baseline
/// checkout's positive count, and both arms are the same language, so a `None` is
/// never ambiguous within a comparison.
///
/// NOTE for `aoa-migrate` (DeadImportFix): do NOT import this scanner to *select*
/// what to remove — that would collapse verify into define. The compiler's
/// `unused_imports` diagnostics are the authority; this stays private to the audit.
pub(super) fn unused_import_proxy_item(
    repo: &Path,
    partition: &SubtreePartition,
) -> Result<Option<PunchItem>, AuditError> {
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    collect_unused_imports(repo, &mut files, 0)?;
    if files.is_empty() {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "likely-unused imports (syntactic proxy)".to_string(),
        kind: FindingKind::UnusedImportProxy,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(files.iter().map(|(_, n)| n).sum(), "imports"),
        plane: None,
        subtree: common_subtree(partition, files.iter().map(|(path, _)| path)),
    }))
}

/// Recursively collect the per-file likely-unused import counts over `.rs`
/// files. Only files with at least one likely-unused import are pushed — the
/// clean majority carries no signal for either the sum or the attribution.
///
/// Kept separate from [`super::collect_source_line_counts`] rather than sharing a walk:
/// that one is multi-language and counts newline bytes, this one is Rust-only and
/// scans tokens. They share only mechanical traversal constraints: bounded depth
/// and reads, hidden/build-output exclusions, and never following symlinks.
fn collect_unused_imports(
    dir: &Path,
    out: &mut Vec<(PathBuf, u64)>,
    depth: usize,
) -> Result<(), AuditError> {
    if depth >= MAX_WALK_DEPTH {
        return Ok(());
    }
    for entry in read_dir(dir)? {
        let entry = entry.map_err(|source| io_err(dir, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        if file_type.is_dir() {
            collect_unused_imports(&path, out, depth + 1)?;
        } else if file_type.is_file() && is_rust_file(&path) {
            // `None` is an oversized file: skipped, not fatal (lossy proxy by
            // contract). A genuine read error propagates.
            if let Some(src) = read_source_capped(&path)? {
                let count = count_unused_imports_in_source(&src);
                if count > 0 {
                    out.push((path, count));
                }
            }
        }
    }
    Ok(())
}

/// The likely-unused import count for a single source file — the testable core of
/// the proxy. Splits `use` statements from the file body, extracts each bound
/// name, and counts those that never appear as an identifier token in the body.
fn count_unused_imports_in_source(src: &str) -> u64 {
    let (bound, body) = split_uses_and_body(src);
    if bound.is_empty() {
        return 0;
    }

    let used: std::collections::HashSet<&str> = identifier_tokens(&body).collect();
    bound
        .iter()
        .filter(|name| !used.contains(name.as_str()))
        .count() as u64
}

/// Partition `src` into the names bound by counted `use` statements and the
/// remaining body text. A `use` statement (optionally multi-line until its `;`)
/// is removed from the body so an import path can never mark *itself* used.
/// `pub use` re-exports are recognized and consumed but contribute no bound names
/// (they are API surface, never compiler-unused).
fn split_uses_and_body(src: &str) -> (Vec<String>, String) {
    let mut bound: Vec<String> = Vec::new();
    let mut body = String::new();
    let mut lines = src.lines();
    while let Some(line) = lines.next() {
        let Some((is_reexport, head)) = use_statement_start(line) else {
            body.push_str(line);
            body.push('\n');
            continue;
        };
        // Accumulate the full statement (until a line containing its `;`).
        let mut stmt = head.to_string();
        while !stmt.contains(';') {
            match lines.next() {
                Some(l) => {
                    stmt.push(' ');
                    stmt.push_str(l);
                }
                None => break, // unterminated: stop rather than loop forever
            }
        }
        if !is_reexport {
            parse_use_tree_text(&stmt, &mut bound);
        }
    }
    (bound, body)
}

/// If `line` begins a `use` statement (after an optional `pub` / `pub(..)`
/// visibility prefix), return `(is_pub_reexport, text_after_the_use_keyword)`.
/// A `//`, `///`, or `//!` comment line trims to `/…`, not `use`/`pub`, so it is
/// never mistaken for an import.
fn use_statement_start(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    let (is_reexport, rest) = match trimmed.strip_prefix("pub") {
        Some(after_pub) => {
            let after_pub = after_pub.trim_start();
            // Skip a `(crate)` / `(in path)` visibility scope if present.
            let after_scope = if after_pub.starts_with('(') {
                &after_pub[after_pub.find(')')? + 1..]
            } else {
                after_pub
            };
            (true, after_scope.trim_start())
        }
        None => (false, trimmed),
    };
    let after_use = rest.strip_prefix("use")?;
    // `use` must be the whole keyword: the next char is whitespace (or the line
    // ends), not an identifier continuation (`useful`, `users`).
    if after_use.is_empty() || after_use.starts_with(char::is_whitespace) {
        Some((is_reexport, after_use.trim_start()))
    } else {
        None
    }
}

/// Extract the names bound by a use-tree (the text after `use`, up to its `;`).
fn parse_use_tree_text(stmt: &str, bound: &mut Vec<String>) {
    let tree = match stmt.find(';') {
        Some(i) => &stmt[..i],
        None => stmt,
    };
    parse_use_tree(tree.trim(), None, bound);
}

/// Recursively collect the leaf names a use-tree binds. `parent` is the path
/// segment a `{ self }` resolves to. Handles `as` aliases, nested brace groups,
/// `*` globs (skipped — untraceable), and `self` (binds the parent module name).
fn parse_use_tree(tree: &str, parent: Option<&str>, bound: &mut Vec<String>) {
    for seg in split_top_level_commas(tree) {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        if let Some(open) = seg.find('{') {
            let prefix = seg[..open].trim_end().trim_end_matches(':');
            let parent_seg = prefix
                .rsplit("::")
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(close) = matching_brace(seg, open) {
                parse_use_tree(&seg[open + 1..close], parent_seg, bound);
            }
        } else if let Some(idx) = seg.rfind(" as ") {
            let alias = seg[idx + 4..].trim();
            // `as _` binds nothing nameable; an empty alias is malformed.
            if alias != "_" && !alias.is_empty() {
                bound.push(alias.to_string());
            }
        } else {
            // `rsplit` always yields at least one element, so the last path
            // segment is the bound name (`a::b::C` -> `C`, bare `C` -> `C`).
            let leaf = seg.rsplit("::").next().unwrap_or(seg).trim();
            match leaf {
                "*" | "" => {}                                      // glob: untraceable
                "self" => bound.extend(parent.map(str::to_string)), // binds the module name
                name => bound.push(name.to_string()),
            }
        }
    }
}

/// Split `s` on commas that sit at brace-nesting depth 0, so a nested `{a, b}`
/// group stays a single segment for the caller to recurse into.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Byte index of the `}` matching the `{` at `open`, or `None` if unbalanced.
fn matching_brace(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (rel, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + rel);
                }
            }
            _ => {}
        }
    }
    None
}

/// Identifier-like tokens (`[A-Za-z0-9_]+`) in `body`. Word-boundary splitting
/// means `Path` does not match inside `PathBuf` — a bound name counts as used
/// only on an exact token match.
fn identifier_tokens(body: &str) -> impl Iterator<Item = &str> {
    body.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::tests::{implicit, tmp, workspace};
    use std::fs;

    #[test]
    fn unused_import_counts_a_plainly_unused_import() {
        let src = "use std::path::Path;\nfn main() {}\n";
        assert_eq!(count_unused_imports_in_source(src), 1);
    }

    #[test]
    fn unused_import_does_not_count_a_used_import() {
        let src = "use std::path::Path;\nfn f(p: &Path) {}\n";
        assert_eq!(count_unused_imports_in_source(src), 0);
    }

    #[test]
    fn unused_import_counts_only_the_unused_member_of_a_braced_group() {
        let src = "use std::path::{Path, PathBuf};\nfn f(p: &Path) {}\n";
        assert_eq!(count_unused_imports_in_source(src), 1, "PathBuf is unused");
    }

    #[test]
    fn unused_import_respects_an_alias() {
        let used = "use std::collections::HashMap as Map;\nfn f() { let _ = Map::new(); }\n";
        assert_eq!(count_unused_imports_in_source(used), 0);
        let unused = "use std::collections::HashMap as Map;\nfn f() {}\n";
        assert_eq!(count_unused_imports_in_source(unused), 1);
    }

    #[test]
    fn unused_import_does_not_match_a_substring_of_another_token() {
        // `Path` must not be considered used by `PathBuf` appearing in the body.
        let src = "use std::path::Path;\nfn f(p: &PathBuf) {}\n";
        assert_eq!(count_unused_imports_in_source(src), 1);
    }

    #[test]
    fn unused_import_does_not_count_an_underscore_alias() {
        // `as _` brings a trait into scope without a nameable binding; it is
        // never "unused" in the syntactic sense and must not be counted.
        let src = "use std::fmt::Write as _;\nfn f() {}\n";
        assert_eq!(count_unused_imports_in_source(src), 0);
    }

    #[test]
    fn unused_import_skips_glob_imports() {
        // A glob binds unknown names; it is untraceable, so it yields no signal.
        let src = "use std::prelude::*;\nfn f() {}\n";
        assert_eq!(count_unused_imports_in_source(src), 0);
    }

    #[test]
    fn unused_import_excludes_pub_use_reexports() {
        // A re-export is API surface, never compiler-unused — excluded outright.
        let src = "pub use crate::inner::Thing;\nfn f() {}\n";
        assert_eq!(count_unused_imports_in_source(src), 0);
        let scoped = "pub(crate) use crate::inner::Thing;\nfn f() {}\n";
        assert_eq!(count_unused_imports_in_source(scoped), 0);
    }

    #[test]
    fn unused_import_handles_self_in_a_braced_group() {
        // `self` binds the module name `io` (used); `Write` is unused.
        let src = "use std::io::{self, Write};\nfn f() { io::stdout(); }\n";
        assert_eq!(count_unused_imports_in_source(src), 1);
    }

    #[test]
    fn unused_import_handles_a_multiline_braced_group() {
        let src = "use std::collections::{\n    HashMap,\n    HashSet,\n};\n\
                   fn f() { let _: HashMap<u8, u8>; }\n";
        assert_eq!(count_unused_imports_in_source(src), 1, "HashSet is unused");
    }

    #[test]
    fn unused_import_ignores_use_in_comments() {
        // Commented-out and doc-comment `use` lines are body text, not imports.
        let src = "// use std::path::Path;\n/// use std::fs::File;\nfn f() {}\n";
        assert_eq!(count_unused_imports_in_source(src), 0);
    }

    #[test]
    fn unused_import_does_not_misfire_on_use_like_identifiers() {
        let src = "fn user() {}\nlet useful = 1;\nfn f() { user(); }\n";
        assert_eq!(count_unused_imports_in_source(src), 0);
    }

    #[test]
    fn unused_import_proxy_item_is_tier3_and_sums_across_files() {
        let dir = tmp("unused-import-item");
        fs::write(dir.join("a.rs"), "use std::path::Path;\nfn main() {}\n").unwrap();
        fs::write(dir.join("b.rs"), "use std::fmt::Debug;\nfn main() {}\n").unwrap();

        let item = unused_import_proxy_item(&dir, &implicit(&dir))
            .unwrap()
            .expect("item");
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "imports");
        assert_eq!(item.measured_cost.value, 2);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unused_import_proxy_abstains_on_a_non_rust_repo() {
        let dir = tmp("unused-import-non-rust");
        // No `.rs` files: nothing to measure -> honest abstention.
        fs::write(dir.join("app.py"), "import os\nimport sys\n").unwrap();
        assert!(unused_import_proxy_item(&dir, &implicit(&dir))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unused_import_proxy_abstains_when_all_imports_are_used() {
        let dir = tmp("unused-import-clean");
        fs::write(
            dir.join("a.rs"),
            "use std::path::Path;\nfn f(p: &Path) {}\n",
        )
        .unwrap();
        assert!(unused_import_proxy_item(&dir, &implicit(&dir))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unused_import_proxy_skips_build_output_dirs() {
        let dir = tmp("unused-import-skip-build");
        fs::write(
            dir.join("a.rs"),
            "use std::path::Path;\nfn f(p: &Path) {}\n",
        )
        .unwrap();
        // A generated file under target/ with an unused import must not be scanned.
        let target = dir.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("gen.rs"), "use std::fmt::Debug;\nfn g() {}\n").unwrap();

        assert!(unused_import_proxy_item(&dir, &implicit(&dir))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unused_import_finding_attributes_to_its_subtree() {
        let (dir, partition) = workspace("attr-unused");
        fs::write(
            dir.join("crates/foo/a.rs"),
            "use std::path::Path;\nfn main() {}\n",
        )
        .unwrap();
        fs::write(
            dir.join("crates/bar/b.rs"),
            "use std::path::Path;\nfn f(p: &Path) {}\n",
        )
        .unwrap();

        let item = unused_import_proxy_item(&dir, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(item.subtree.as_deref(), Some("crates/foo"));
        fs::remove_dir_all(&dir).ok();
    }
}
