//! The invariant-discoverability probe: can an agent find the rules this
//! repository expects it to follow?
//!
//! One probe, one module. Like the verification probe it is a fixed
//! well-known-name scan — policy files, an agent-context doc, a CODEOWNERS map
//! — and it asks only whether such a surface exists, never whether the rules
//! it states are sound.

use std::path::{Path, PathBuf};

use aoa_metrics::SubtreePartition;

use super::{common_subtree, io_err, package_roots, read_dir, MAX_WALK_DEPTH, SKIP_DIRS};
use crate::error::AuditError;
use crate::punch::{FindingKind, MeasuredCost, PunchItem};
use crate::tier::Tier;

/// Rule/invariant marker filenames whose presence means the project's *declared
/// conventions* are statically discoverable before an agent edits — a policy
/// file, an agent-context doc, a lint/format config, a pre-commit config, or a
/// CODEOWNERS. Matched case-insensitively, the mechanical equivalent of the
/// verification family's `TEST_CONFIG_MARKERS`: presence of a well-known
/// rule-file marker, never a judgment of the *content* of the rules (ZFC). The
/// `CONTRIBUTING*` doc and `.eslintrc*` family (many extensions) are matched by
/// prefix in [`is_invariant_file`] instead. README is deliberately *excluded*:
/// it is the navigability anchor ([`super::navigability_sites`]), a distinct construct.
const INVARIANT_FILE_MARKERS: &[&str] = &[
    "aoa-policy.yaml",
    "aoa-policy.yml",
    "agents.md",
    "claude.md",
    ".editorconfig",
    "rustfmt.toml",
    ".rustfmt.toml",
    "clippy.toml",
    ".clippy.toml",
    ".pre-commit-config.yaml",
    "codeowners",
];

/// Directory that, by presence alone, declares a project's policy/conventions
/// (the AOA policy tree). A hidden dir, so — like `.github` for the verification
/// family — it is reached only by the repo-global probe, never the per-package
/// walk (which prunes hidden dirs).
const INVARIANT_DIR: &str = ".aoa";

/// Conventional non-root CODEOWNERS locations (relative to the repo root). A root
/// `CODEOWNERS` is already caught by [`INVARIANT_FILE_MARKERS`]; these two cover
/// the `.github/` and `docs/` placements, probed by filename like `CI_FILES`.
const CODEOWNERS_PATHS: &[&str] = &[".github/CODEOWNERS", "docs/CODEOWNERS"];

/// The package roots under `repo` for which the project's *declared rules /
/// invariants* are not statically discoverable before editing — the static
/// "invariant reachability" leg of the audit (sibling to [`super::verification_sites`]).
///
/// Rules are discoverable for a package root when an agent could, by reading the
/// tree alone, find the project's declared conventions before touching code. Two
/// discovery scopes satisfy it, mirroring [`super::verification_sites`]:
///
/// - **Repo-global** (short-circuits the whole probe to *empty*): a `.aoa` policy
///   dir, a non-root CODEOWNERS ([`CODEOWNERS_PATHS`]), or any root-level
///   [`INVARIANT_FILE_MARKERS`] file. A repo-wide rule source is discoverable
///   from one front-door place, so no root is a site.
/// - **Per-package-local**: an [`INVARIANT_FILE_MARKERS`] file found by a bounded
///   walk *under* the root (skipping build-output and hidden *directories* and
///   never following symlinks). Unlike `verification::has_local_verification`, a leading-dot
///   *file* is NOT skipped — rule files are commonly dotfiles (`.editorconfig`,
///   `.eslintrc.json`) — only hidden directories are pruned.
///
/// This is *reachability only* — presence of a declared-rule marker, never a
/// judgment of the rules' adequacy or content (that semantic call belongs to a
/// model, not this probe; ZFC). Every signal is a mechanical filesystem /
/// documented-marker check, biased toward finding discoverability (the
/// conservative direction for an advisory probe): e.g. a `.aoa` dir holding only
/// traces still counts. Born advisory like its siblings; the audit reports only
/// the count of roots with no discoverable rules.
pub fn invariant_sites(repo: &Path) -> Result<Vec<PathBuf>, AuditError> {
    // A repo-global rule source is discoverable for every root at once.
    if has_repo_global_invariants(repo)? {
        return Ok(Vec::new());
    }
    let mut sites = Vec::new();
    for root in package_roots(repo)? {
        if !has_local_invariants(&root)? {
            sites.push(root);
        }
    }
    Ok(sites)
}

/// Count package roots with no statically discoverable declared rules/invariants.
/// The count is exactly the length of [`invariant_sites`].
pub(super) fn invariant_discoverability_item(
    repo: &Path,
    partition: &SubtreePartition,
) -> Result<Option<PunchItem>, AuditError> {
    let sites = invariant_sites(repo)?;
    if sites.is_empty() {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "package roots without discoverable rules/invariants".to_string(),
        kind: FindingKind::InvariantDiscoverability,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(sites.len() as u64, "package roots"),
        plane: None,
        subtree: common_subtree(partition, sites.iter()),
        size_outliers: None,
    }))
}

/// Whether a repo-wide rule source is discoverable: a `.aoa` policy dir, a
/// non-root CODEOWNERS, or any root-level invariant-marker file. Any one makes
/// the project's declared conventions reachable for the whole repo at once.
fn has_repo_global_invariants(repo: &Path) -> Result<bool, AuditError> {
    if repo.join(INVARIANT_DIR).is_dir() || has_nonroot_codeowners(repo) {
        return Ok(true);
    }
    has_root_invariant_marker(repo)
}

/// Whether any [`CODEOWNERS_PATHS`] file exists. `.is_file()` follows symlinks, a
/// fixed-name one-level probe that cannot escape the tree (like [`super::has_manifest`]).
fn has_nonroot_codeowners(repo: &Path) -> bool {
    CODEOWNERS_PATHS.iter().any(|rel| repo.join(rel).is_file())
}

/// Whether any immediate entry of `repo` is an invariant-marker *file*. Scans the
/// root level only — a repo-global rule source lives at the front door. Hidden
/// files are evaluated (rule files are commonly dotfiles); directory descent is
/// the per-package walk's job.
fn has_root_invariant_marker(repo: &Path) -> Result<bool, AuditError> {
    for entry in read_dir(repo)? {
        let entry = entry.map_err(|source| io_err(repo, source))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        if file_type.is_file() && is_invariant_file(&entry.file_name().to_string_lossy()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a declared-rule marker is reachable *within* `root` by a walk bounded
/// by [`super::SKIP_DIRS`], hidden-*directory* exclusions, and [`super::MAX_WALK_DEPTH`].
/// Short-circuits on the first hit. Never follows symlinks.
///
/// Divergence from `verification::has_local_verification` by design: a leading-dot *file* is
/// NOT skipped here, because rule files are overwhelmingly dotfiles
/// (`.editorconfig`, `.eslintrc.json`, `.pre-commit-config.yaml`); only hidden
/// *directories* are pruned. The hidden `.aoa` dir is therefore a repo-global
/// signal only ([`has_repo_global_invariants`]), never found by this walk.
fn has_local_invariants(root: &Path) -> Result<bool, AuditError> {
    has_local_invariants_bounded(root, 0)
}

fn has_local_invariants_bounded(root: &Path, depth: usize) -> Result<bool, AuditError> {
    if depth >= MAX_WALK_DEPTH {
        return Ok(false);
    }
    for entry in read_dir(root)? {
        let entry = entry.map_err(|source| io_err(root, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        if file_type.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            if has_local_invariants_bounded(&path, depth + 1)? {
                return Ok(true);
            }
        } else if file_type.is_file() && is_invariant_file(&name) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `name` is a declared-rule marker: an exact (case-insensitive)
/// [`INVARIANT_FILE_MARKERS`] match, a `CONTRIBUTING*` doc, or a member of the
/// `.eslintrc*` config family. A documented well-known set; matching is mechanical
/// name presence, never a read of the rule content.
fn is_invariant_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    INVARIANT_FILE_MARKERS.contains(&lower.as_str())
        || lower.starts_with("contributing")
        || lower.starts_with(".eslintrc")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::tests::{implicit, tmp, workspace};
    use std::fs;

    #[test]
    fn invariant_site_when_no_rules_are_discoverable() {
        // A package root with source but no policy/agent-context/lint-format/
        // pre-commit/CODEOWNERS marker anywhere: the project's declared
        // conventions are undiscoverable before editing -> a site. A README is
        // navigation, not a declared rule, so it does not satisfy this probe.
        let dir = tmp("inv-none");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(sites.contains(&dir), "root has no discoverable rules");
        assert_eq!(sites.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_discoverable_via_a_root_policy_file() {
        let dir = tmp("inv-policy");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("aoa-policy.yaml"), "version: 1\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(
            sites.is_empty(),
            "a root policy file makes rules discoverable repo-wide"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_discoverable_via_a_root_editorconfig() {
        // A lint/format config is a declared convention; .editorconfig is a
        // dotfile, so the root-level scan must see hidden files.
        let dir = tmp("inv-editorconfig");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join(".editorconfig"), "root = true\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(sites.is_empty(), "a root .editorconfig is a declared rule");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_discoverable_via_the_aoa_dir() {
        let dir = tmp("inv-aoa-dir");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir_all(dir.join(".aoa")).unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(
            sites.is_empty(),
            "a .aoa policy dir makes rules discoverable"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_discoverable_via_codeowners_in_github() {
        // CODEOWNERS conventionally lives under .github/; that hidden dir is only
        // reachable through the repo-global probe, not the per-package walk.
        let dir = tmp("inv-codeowners");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let gh = dir.join(".github");
        fs::create_dir_all(&gh).unwrap();
        fs::write(gh.join("CODEOWNERS"), "* @team\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(
            sites.is_empty(),
            ".github/CODEOWNERS makes ownership rules discoverable"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_reachable_via_a_local_dotfile_config() {
        // Per-package leg: a member crate carrying its OWN dotfile config makes
        // its rules reachable. Unlike the verification walk, a leading-dot FILE
        // is NOT skipped (rule files are commonly dotfiles); only hidden DIRS are.
        // No repo-global marker, so per-package reachability is what decides.
        let dir = tmp("inv-local-dotfile");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        let foo = dir.join("crates").join("foo");
        fs::create_dir_all(&foo).unwrap();
        fs::write(foo.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(foo.join(".eslintrc.json"), "{}\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(
            !sites.contains(&foo),
            "foo's local .eslintrc.json is a discoverable rule (dotfile not skipped)"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_partial_across_member_crates() {
        // Monorepo, no repo-global marker: member foo carries a local rustfmt.toml
        // (covered, and confers reachability to the root via the recursive walk),
        // member bar carries nothing -> only bar is a site.
        let dir = tmp("inv-partial");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let foo = dir.join("crates").join("foo");
        fs::create_dir_all(&foo).unwrap();
        fs::write(foo.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(foo.join("rustfmt.toml"), "edition = \"2021\"\n").unwrap();

        let bar = dir.join("crates").join("bar");
        fs::create_dir_all(&bar).unwrap();
        fs::write(bar.join("Cargo.toml"), "[package]\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(
            sites.contains(&bar),
            "bar has no discoverable rules -> a site"
        );
        assert!(!sites.contains(&foo), "foo's rustfmt.toml is discoverable");
        assert!(
            !sites.contains(&dir),
            "root is reachable via foo's rule file"
        );
        assert_eq!(sites.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_walk_skips_build_output_and_hidden_dirs() {
        // A rule file buried in target/ or a hidden dir must NOT confer
        // reachability — those are not "the codebase". (The repo-global probe
        // saw no root-level marker, so this exercises the per-package walk.)
        let dir = tmp("inv-skip");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let target = dir.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(".editorconfig"), "root = true\n").unwrap();
        let hidden = dir.join(".cache");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("rustfmt.toml"), "x = 1\n").unwrap();

        let sites = invariant_sites(&dir).unwrap();
        assert!(
            sites.contains(&dir),
            "rules under target/ or hidden dirs do not count"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn invariant_walk_does_not_follow_symlinked_dirs() {
        // A rule file reachable only by following a symlink out of the package
        // must NOT confer reachability — mirrors the symlink non-follow invariant
        // the sibling walks already guard.
        use std::os::unix::fs::symlink;
        let base = tmp("inv-symlink");
        let repo = base.join("repo");
        let outside = base.join("outside");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(outside.join(".editorconfig"), "root = true\n").unwrap();
        symlink(&outside, repo.join("link")).unwrap();

        let sites = invariant_sites(&repo).unwrap();
        assert!(
            sites.contains(&repo),
            "a rule file reachable only through a symlink does not count"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn invariant_discoverability_item_is_tier3_and_counts_sites() {
        let dir = tmp("inv-item");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = invariant_discoverability_item(&dir, &implicit(&dir))
            .unwrap()
            .expect("item");
        assert_eq!(item.kind, FindingKind::InvariantDiscoverability);
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "package roots");
        assert_eq!(item.measured_cost.value, 1);
        assert!(item.plane.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_invariant_item_when_all_roots_are_discoverable() {
        let dir = tmp("inv-item-none");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("AGENTS.md"), "# conventions\n").unwrap();

        assert!(invariant_discoverability_item(&dir, &implicit(&dir))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invariant_finding_attributes_to_its_subtree() {
        let (dir, partition) = workspace("attr-inv");
        // foo's rustfmt.toml covers foo AND (via the recursive walk) the root;
        // bar alone has no discoverable rules.
        fs::write(dir.join("crates/foo/rustfmt.toml"), "edition = \"2021\"\n").unwrap();

        let item = invariant_discoverability_item(&dir, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(item.subtree.as_deref(), Some("crates/bar"));
        fs::remove_dir_all(&dir).ok();
    }
}
