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
use crate::punch::{MeasuredCost, PunchItem};
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
    Ok(items)
}

/// The package roots under `repo` that lack a navigability anchor (README):
/// the repo root, plus every immediate child directory carrying a build
/// manifest, minus those that already have a README.
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
        if file_type.is_dir() && has_manifest(&path) {
            roots.push(path);
        }
    }

    roots.retain(|root| !has_readme(root));
    Ok(roots)
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
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(outliers as u64, "outlier files"),
        plane: None,
    }))
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
}
