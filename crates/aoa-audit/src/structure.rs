//! Code-structure best-practices audit family.
//!
//! These checks surface *measured facts* about a repo's code-infrastructure —
//! the structure, organization, and navigability an agent builds on — as
//! [`PunchItem`]s alongside the enforcement-plane and budget checks. They are
//! the grounded signal R0's repo-delta arm needs to ask "how much better
//! organized is the migrated checkout?".
//!
//! Every check here is born [`Tier::Tier3`] (asserted-but-unsupported). A
//! structure measure is a *fact*, not an evidence-backed best-practice: it does
//! not become gating until external-outcome correlation (revert / incident /
//! review-acceptance, the R9c discipline in `aoa-gap`) promotes it. We therefore
//! report only neutral, measured counts — never an opinion-bearing "deficiency"
//! — so the audit *verifies* a pre-registered spec rather than *defining* one
//! (anti-Goodhart; see `docs/r0_runbook.md`).

use std::path::{Path, PathBuf};

use crate::error::AuditError;
use crate::punch::{FindingKind, MeasuredCost, PunchItem};
use crate::tier::Tier;

/// Largest single source file read while counting lines. A hand-written module
/// is virtually never this large; the cap only trips pathological or hostile
/// input (mirrors aoa-scip-graph's bounded read).
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

/// Build-manifest filenames that mark a directory as a package root. A directory
/// carrying one of these is unambiguously a package (mechanical, not a quality
/// judgment) — the same well-known-path style as the enforcement-plane probes.
const MANIFEST_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "setup.py",
    "go.mod",
    "pom.xml",
    "build.gradle",
];

/// Directory names that conventionally hold workspace member packages one level
/// deeper (`crates/foo/Cargo.toml`, `packages/bar/package.json`). A well-known
/// monorepo-layout list — the language-agnostic, mechanical equivalent of
/// parsing each ecosystem's `[workspace] members`, in the same documented
/// well-known-name style as [`MANIFEST_MARKERS`] and [`SKIP_DIRS`]. Members are
/// discovered exactly one level inside such a dir; deeper nesting is out of
/// scope (see [`navigability_sites`]).
const WORKSPACE_CONTAINER_DIRS: &[&str] = &["crates", "packages", "apps", "libs"];

/// Source-file extensions counted for the module-size measure. A documented,
/// well-known set — extension matching is mechanical, like the plane candidates.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "h", "cpp", "hpp", "cc", "rb", "php",
    "swift", "kt", "scala", "cs",
];

/// Directory names skipped while walking: build output and vendored trees are
/// not "the codebase" and would pollute the self-calibrating median. Hidden
/// directories are skipped separately (and symlinks are never followed).
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "vendor",
    "dist",
    "build",
    "__pycache__",
];

/// Minimum number of source files required before the module-size measure is
/// meaningful: a median computed from a handful of files cannot self-calibrate,
/// so below this the check abstains (emits nothing) rather than assert an
/// outlier from noise.
const MIN_FILES_FOR_MEDIAN: usize = 5;

/// Run the code-structure audit family over `repo`, returning measured-fact
/// punch items (each born [`Tier::Tier3`]). `size_outlier_k` is the caller's
/// documented multiplier for the module-size measure.
pub(crate) fn structure_items(
    repo: &Path,
    size_outlier_k: f64,
) -> Result<Vec<PunchItem>, AuditError> {
    let mut items = Vec::new();
    if let Some(item) = navigability_anchor_item(repo)? {
        items.push(item);
    }
    if let Some(item) = module_size_outlier_item(repo, size_outlier_k)? {
        items.push(item);
    }
    if let Some(item) = unused_import_proxy_item(repo)? {
        items.push(item);
    }
    Ok(items)
}

/// The package roots under `repo` that lack a navigability anchor (README):
/// the repo root, plus every immediate child directory carrying a build
/// manifest, plus workspace member packages nested one level inside a
/// well-known container dir (`crates/foo/`, `packages/bar/`; see
/// [`WORKSPACE_CONTAINER_DIRS`]) — minus any that already have a README.
///
/// Discovery is deliberately *bounded*, not a full-tree manifest sweep: an
/// unbounded walk would fold in trybuild test-fixture crates, `examples/`
/// sub-crates, and partially-vendored trees, inflating the count past the
/// construct it names ("workspace member crate") and — because `aoa-migrate`
/// *writes* READMEs into these sites — writing anchors into test fixtures. The
/// container-dir convention captures real members while excluding those.
///
/// This is the per-site finding behind the navigability measure. The audit
/// reports only its *count* (a measured fact), but `aoa-migrate` consumes the
/// concrete sites so a migration fixes *exactly* what the audit measured —
/// there is one package-root walk, not two that can drift apart.
pub fn navigability_sites(repo: &Path) -> Result<Vec<PathBuf>, AuditError> {
    let mut roots: Vec<PathBuf> = vec![repo.to_path_buf()];
    for entry in read_dir(repo)? {
        let entry = entry.map_err(|source| io_err(repo, source))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        // `file_type` does not follow symlinks, so a symlinked dir is skipped.
        if !file_type.is_dir() {
            continue;
        }
        // A `crates/` (etc.) dir holds members one level deeper; scan its own
        // immediate children for manifests. Membership is by directory name —
        // the same mechanical well-known-name match as elsewhere in the family.
        // Done before the manifest push so `path` need not be cloned, and so a
        // dir that is *both* a container and a package itself contributes both
        // its members and itself.
        if is_workspace_container(&path) {
            collect_container_members(&path, &mut roots)?;
        }
        if has_manifest(&path) {
            roots.push(path);
        }
    }

    roots.retain(|root| !has_readme(root));
    Ok(roots)
}

/// Whether `dir`'s name is a conventional workspace-container dir
/// ([`WORKSPACE_CONTAINER_DIRS`]).
fn is_workspace_container(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| WORKSPACE_CONTAINER_DIRS.contains(&n))
}

/// Push every immediate child of `container` that carries a build manifest. One
/// level only — `crates/foo/Cargo.toml` is a member, `crates/foo/bar/Cargo.toml`
/// is not (deeper nesting is out of scope). Never follows symlinked dirs.
fn collect_container_members(container: &Path, out: &mut Vec<PathBuf>) -> Result<(), AuditError> {
    for entry in read_dir(container)? {
        let entry = entry.map_err(|source| io_err(container, source))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        // `file_type` does not follow symlinks, so a symlinked member is skipped.
        if file_type.is_dir() && has_manifest(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Count package roots that have no README. A package without a navigability
/// anchor is a measured fact about how findable its entry point is. The count
/// is exactly the length of [`navigability_sites`] — the migration acts on the
/// same set.
fn navigability_anchor_item(repo: &Path) -> Result<Option<PunchItem>, AuditError> {
    let missing = navigability_sites(repo)?.len();
    if missing == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "package roots without a navigability anchor (README)".to_string(),
        kind: FindingKind::NavigabilityAnchor,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(missing as u64, "package roots"),
        plane: None,
    }))
}

/// Count source files whose line count exceeds `k ×` the repo's *own* median
/// source-file line count. Self-calibrating: the threshold is the repo's own
/// distribution, not an external magic size, so the measure asserts no absolute
/// best-practice. Abstains below [`MIN_FILES_FOR_MEDIAN`] files.
fn module_size_outlier_item(repo: &Path, k: f64) -> Result<Option<PunchItem>, AuditError> {
    let mut line_counts: Vec<u64> = Vec::new();
    collect_source_line_counts(repo, &mut line_counts)?;

    if line_counts.len() < MIN_FILES_FOR_MEDIAN {
        return Ok(None);
    }

    line_counts.sort_unstable();
    let median = median(&line_counts);
    // A zero median (a repo of empty source files) has no scale to compare
    // against — abstain rather than divide a threshold into nothing.
    if median == 0 {
        return Ok(None);
    }

    // Line counts are capped by MAX_SOURCE_BYTES (~8M lines max), far below
    // f64's 2^53 exact-integer range, so these casts lose no precision; `k` is
    // fractional, so the comparison must be in f64.
    let threshold = median as f64 * k;
    let outliers = line_counts
        .iter()
        .filter(|&&n| n as f64 > threshold)
        .count();
    if outliers == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: format!("source files exceeding {k:.1}x the repo median size"),
        kind: FindingKind::ModuleSizeOutlier,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(outliers as u64, "outlier files"),
        plane: None,
    }))
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
fn unused_import_proxy_item(repo: &Path) -> Result<Option<PunchItem>, AuditError> {
    let mut count: u64 = 0;
    collect_unused_imports(repo, &mut count)?;
    if count == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "likely-unused imports (syntactic proxy)".to_string(),
        kind: FindingKind::UnusedImportProxy,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(count, "imports"),
        plane: None,
    }))
}

/// Recursively sum the per-file likely-unused import count over `.rs` files.
///
/// Kept separate from [`collect_source_line_counts`] rather than sharing a walk:
/// that one is multi-language and counts newline bytes, this one is Rust-only and
/// scans tokens — the only genuinely shared invariant is the bounded read, which
/// [`read_source_capped`] carries. Same skip-hidden / skip-build-output /
/// never-follow-symlinks discipline as the rest of the family.
fn collect_unused_imports(dir: &Path, count: &mut u64) -> Result<(), AuditError> {
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
            collect_unused_imports(&path, count)?;
        } else if file_type.is_file() && is_rust_file(&path) {
            // `None` is an oversized file: skipped, not fatal (lossy proxy by
            // contract). A genuine read error propagates.
            if let Some(src) = read_source_capped(&path)? {
                *count += count_unused_imports_in_source(&src);
            }
        }
    }
    Ok(())
}

/// Whether `path` is a Rust source file.
fn is_rust_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("rs")
}

/// Read `path` as UTF-8 text, returning `None` if it exceeds the byte cap (the
/// caller skips it). Decodes lossily so a stray non-UTF-8 byte cannot abort the
/// whole walk; only a genuine IO error propagates. Mirrors [`count_lines`]'s
/// bounded read — the one invariant the import scan shares with the size measure.
fn read_source_capped(path: &Path) -> Result<Option<String>, AuditError> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|source| io_err(path, source))?;
    let mut raw = Vec::new();
    let read = file
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|source| io_err(path, source))?;
    if read as u64 > MAX_SOURCE_BYTES {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&raw).into_owned()))
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

/// Median of a pre-sorted, non-empty slice. The even case averages the two
/// middle values via [`u64::midpoint`] (overflow-safe, rounds down).
fn median(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        u64::midpoint(sorted[n / 2 - 1], sorted[n / 2])
    }
}

/// Whether `dir` contains any build manifest. `exists()` follows symlinks, so a
/// symlinked manifest still marks the directory as a real package root — the
/// intended semantic. (Directory *traversal* never follows symlinks; this is a
/// one-level existence probe of a fixed filename, so it cannot amplify or
/// escape the tree.)
fn has_manifest(dir: &Path) -> bool {
    MANIFEST_MARKERS.iter().any(|m| dir.join(m).exists())
}

/// Whether `dir` contains a README (any `readme.*` / bare `readme`, case-insensitive).
fn has_readme(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_ascii_lowercase();
        name == "readme" || name.starts_with("readme.")
    })
}

/// Recursively collect line counts of source files under `dir`, skipping hidden
/// and build-output directories and never following symlinks (matching
/// aoa-scip-graph's best-effort walk). An oversized single file is skipped, not
/// fatal; a genuine read error propagates.
fn collect_source_line_counts(dir: &Path, out: &mut Vec<u64>) -> Result<(), AuditError> {
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
            collect_source_line_counts(&path, out)?;
        } else if file_type.is_file() && is_source_file(&path) {
            // `None` is an oversized file: skipped, not fatal (the scan is a
            // lossy structural proxy by contract). A genuine read error
            // propagates.
            if let Some(n) = count_lines(&path)? {
                out.push(n);
            }
        }
    }
    Ok(())
}

/// Whether `path` has a recognized source extension.
fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCE_EXTENSIONS.contains(&e))
}

/// Count newline bytes in `path`, returning `None` if the file exceeds the byte
/// cap (the caller skips it). Reads raw bytes and never decodes UTF-8, so a
/// binary or non-UTF-8 file carrying a source extension (a Latin-1 `.c`, an
/// embedded blob) is counted rather than aborting the whole scan — the measure
/// is a lossy structural proxy by contract. Only a genuine IO error
/// (permissions, vanished file) propagates.
fn count_lines(path: &Path) -> Result<Option<u64>, AuditError> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|source| io_err(path, source))?;
    let mut raw = Vec::new();
    let read = file
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|source| io_err(path, source))?;
    if read as u64 > MAX_SOURCE_BYTES {
        return Ok(None);
    }
    Ok(Some(raw.iter().filter(|&&b| b == b'\n').count() as u64))
}

/// `read_dir` with the crate's path-carrying IO error (no `From<io::Error>`
/// exists because [`AuditError::Io`] carries the path).
fn read_dir(dir: &Path) -> Result<std::fs::ReadDir, AuditError> {
    std::fs::read_dir(dir).map_err(|source| io_err(dir, source))
}

fn io_err(path: &Path, source: std::io::Error) -> AuditError {
    AuditError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aoa-structure-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn median_handles_odd_and_even() {
        assert_eq!(median(&[1, 2, 3]), 2);
        assert_eq!(median(&[1, 2, 3, 5]), 2); // (2+3)/2 floored
    }

    #[test]
    fn navigability_sites_lists_each_root_without_a_readme() {
        let dir = tmp("nav-sites");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        // A child package missing a README is a site; one with a README is not.
        let missing = dir.join("crate-a");
        fs::create_dir_all(&missing).unwrap();
        fs::write(missing.join("Cargo.toml"), "[package]\n").unwrap();
        let present = dir.join("crate-b");
        fs::create_dir_all(&present).unwrap();
        fs::write(present.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(present.join("README.md"), "# b\n").unwrap();

        let sites = navigability_sites(&dir).unwrap();
        assert!(sites.contains(&dir), "repo root lacks a README -> a site");
        assert!(sites.contains(&missing), "crate-a lacks a README -> a site");
        assert!(
            !sites.contains(&present),
            "crate-b has a README -> not a site"
        );
        // The count the audit reports is exactly the number of sites.
        assert_eq!(sites.len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_item_when_root_lacks_readme() {
        let dir = tmp("nav-missing");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = navigability_anchor_item(&dir).unwrap().expect("item");
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "package roots");
        assert_eq!(item.measured_cost.value, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_navigability_item_when_root_has_readme() {
        let dir = tmp("nav-present");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();

        assert!(navigability_anchor_item(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_counts_manifest_child_packages() {
        let dir = tmp("nav-children");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        // A child package (has a manifest) without a README is counted.
        let pkg = dir.join("crate-a");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("Cargo.toml"), "[package]\n").unwrap();
        // A plain child dir (no manifest) is NOT a package root and is ignored.
        let plain = dir.join("docs");
        fs::create_dir_all(&plain).unwrap();

        let item = navigability_anchor_item(&dir).unwrap().expect("item");
        assert_eq!(
            item.measured_cost.value, 1,
            "only the manifest child counts"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_discovers_a_nested_member_crate() {
        // The motivating case: a Cargo workspace whose members live one level
        // deeper under crates/. crates/foo/Cargo.toml without a README is a site.
        let dir = tmp("nav-nested-member");
        fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        let member = dir.join("crates").join("foo");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join("Cargo.toml"), "[package]\n").unwrap();

        let sites = navigability_sites(&dir).unwrap();
        assert!(
            sites.contains(&member),
            "crates/foo is a member crate lacking a README -> a site"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_discovers_members_in_a_js_container_dir() {
        // Multi-language: packages/bar/package.json is a member just like a crate.
        let dir = tmp("nav-js-container");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        let member = dir.join("packages").join("bar");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join("package.json"), "{}\n").unwrap();

        let sites = navigability_sites(&dir).unwrap();
        assert!(
            sites.contains(&member),
            "packages/bar is a member -> a site"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_excludes_a_member_with_a_readme() {
        let dir = tmp("nav-member-readme");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        let member = dir.join("crates").join("foo");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(member.join("README.md"), "# foo\n").unwrap();

        let sites = navigability_sites(&dir).unwrap();
        assert!(
            !sites.contains(&member),
            "a member with a README is not a site"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_does_not_discover_manifests_outside_a_container_dir() {
        // The C1 guard: a manifest nested under a NON-container dir (a trybuild
        // test fixture) must NOT be a site — discovery is bounded to known
        // workspace-container dirs, so migrate never writes a README into it.
        let dir = tmp("nav-bounded");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        // One level under a NON-container dir: if the container-name guard were
        // removed, the one-level member scan WOULD reach and push this. The
        // guard is what excludes it — so this test fails if the bound is lost.
        let fixture = dir.join("tests").join("bad");
        fs::create_dir_all(&fixture).unwrap();
        fs::write(fixture.join("Cargo.toml"), "[package]\n").unwrap();

        let sites = navigability_sites(&dir).unwrap();
        assert!(
            !sites.contains(&fixture),
            "a fixture crate outside a container dir must not be a site"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_does_not_recurse_deeper_than_one_container_level() {
        // crates/foo/bar/Cargo.toml (two levels inside crates/) is out of scope.
        let dir = tmp("nav-too-deep");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        let deep = dir.join("crates").join("foo").join("bar");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("Cargo.toml"), "[package]\n").unwrap();

        let sites = navigability_sites(&dir).unwrap();
        assert!(
            !sites.contains(&deep),
            "a manifest two levels inside a container dir is out of scope"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn navigability_does_not_follow_a_symlinked_member() {
        use std::os::unix::fs::symlink;
        let base = tmp("nav-symlink");
        let repo = base.join("repo");
        let outside = base.join("outside");
        fs::create_dir_all(repo.join("crates")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(repo.join("README.md"), "# root\n").unwrap();
        // An out-of-repo package symlinked in as a member must not be a site.
        fs::write(outside.join("Cargo.toml"), "[package]\n").unwrap();
        symlink(&outside, repo.join("crates").join("escaped")).unwrap();

        let sites = navigability_sites(&repo).unwrap();
        assert!(
            !sites.iter().any(|s| s.ends_with("escaped")),
            "a symlinked member dir must not be followed"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn size_outlier_flags_a_file_far_above_the_median() {
        let dir = tmp("size-outlier");
        for i in 0..6 {
            fs::write(dir.join(format!("m{i}.rs")), "x\n".repeat(10)).unwrap();
        }
        fs::write(dir.join("huge.rs"), "x\n".repeat(200)).unwrap();

        let item = module_size_outlier_item(&dir, 4.0).unwrap().expect("item");
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "outlier files");
        assert_eq!(item.measured_cost.value, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_size_outlier_when_files_are_uniform() {
        let dir = tmp("size-uniform");
        for i in 0..8 {
            fs::write(dir.join(format!("m{i}.rs")), "x\n".repeat(20)).unwrap();
        }
        assert!(module_size_outlier_item(&dir, 4.0).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_outlier_abstains_below_the_minimum_file_count() {
        let dir = tmp("size-too-few");
        // Two files, one much larger: too few to self-calibrate a median.
        fs::write(dir.join("a.rs"), "x\n").unwrap();
        fs::write(dir.join("b.rs"), "x\n".repeat(500)).unwrap();
        assert!(module_size_outlier_item(&dir, 4.0).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_outlier_abstains_when_median_is_zero() {
        let dir = tmp("size-zero-median");
        // Enough files to clear the count floor, but all empty (0 newlines):
        // the median is 0 and there is no scale to compare against.
        for i in 0..6 {
            fs::write(dir.join(format!("m{i}.rs")), "").unwrap();
        }
        assert!(module_size_outlier_item(&dir, 4.0).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_measure_counts_non_utf8_source_without_aborting() {
        let dir = tmp("size-non-utf8");
        for i in 0..6 {
            fs::write(dir.join(format!("m{i}.rs")), "x\n".repeat(10)).unwrap();
        }
        // A source-extension file with invalid UTF-8 must be counted by bytes,
        // not abort the scan with an InvalidData error.
        fs::write(dir.join("latin1.c"), [0xff, b'\n', 0xfe, b'\n']).unwrap();

        let mut counts = Vec::new();
        collect_source_line_counts(&dir, &mut counts).unwrap();
        assert_eq!(counts.len(), 7, "non-utf8 file must be counted, not fatal");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walk_skips_hidden_directories() {
        let dir = tmp("size-hidden");
        for i in 0..6 {
            fs::write(dir.join(format!("m{i}.rs")), "x\n".repeat(10)).unwrap();
        }
        // A hidden dir (e.g. .git) holding a huge source file must not be walked.
        let hidden = dir.join(".git");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("hook.rs"), "x\n".repeat(9999)).unwrap();

        let mut counts = Vec::new();
        collect_source_line_counts(&dir, &mut counts).unwrap();
        assert_eq!(counts.len(), 6, "hidden dir must not be traversed");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_measure_skips_build_output_dirs() {
        let dir = tmp("size-skip-build");
        for i in 0..6 {
            fs::write(dir.join(format!("m{i}.rs")), "x\n".repeat(10)).unwrap();
        }
        // A vendored/generated huge file under target/ must not skew the median
        // or count as an outlier.
        let target = dir.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("gen.rs"), "x\n".repeat(5000)).unwrap();

        assert!(module_size_outlier_item(&dir, 4.0).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_symlinked_dirs() {
        use std::os::unix::fs::symlink;
        let base = tmp("symlink");
        let repo = base.join("repo");
        let outside = base.join("outside");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&outside).unwrap();
        for i in 0..6 {
            fs::write(repo.join(format!("m{i}.rs")), "x\n".repeat(10)).unwrap();
        }
        fs::write(outside.join("escaped.rs"), "x\n".repeat(9999)).unwrap();
        symlink(&outside, repo.join("link")).unwrap();

        // If the symlink were followed, escaped.rs would appear and skew the
        // median / produce an outlier. It must not.
        let mut counts = Vec::new();
        collect_source_line_counts(&repo, &mut counts).unwrap();
        assert_eq!(counts.len(), 6, "symlinked dir must not be traversed");
        fs::remove_dir_all(&base).ok();
    }

    // --- unused-import syntactic proxy ---

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

        let item = unused_import_proxy_item(&dir).unwrap().expect("item");
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
        assert!(unused_import_proxy_item(&dir).unwrap().is_none());
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
        assert!(unused_import_proxy_item(&dir).unwrap().is_none());
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

        assert!(unused_import_proxy_item(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
