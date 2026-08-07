//! The verification-reachability probe: can an agent find how to run this
//! repository's tests?
//!
//! One probe, one module. The scan is a fixed set of well-known names — test
//! directories, runner configs, CI files, documented commands — so everything
//! it knows lives in the consts at the top and nothing here judges whether the
//! command it found is any good.

use std::path::{Path, PathBuf};

use aoa_metrics::SubtreePartition;

use super::{
    common_subtree, io_err, is_rust_file, package_roots, read_dir, read_source_capped,
    MAX_WALK_DEPTH, SKIP_DIRS,
};
use crate::error::AuditError;
use crate::punch::{FindingKind, MeasuredCost, PunchItem};
use crate::tier::Tier;

/// Directory names that conventionally hold a package's tests
/// (`tests/` for Rust/Python, `__tests__/` for JS, `spec/` for Ruby/JS). A
/// documented well-known set — the same mechanical name match as
/// [`super::WORKSPACE_CONTAINER_DIRS`]; the presence of such a dir is a discoverable
/// verification entrypoint, not a quality judgment.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec"];

/// Test-runner configuration filenames. A directory carrying one documents how
/// the project's tests are run — a reachable verification entrypoint by file
/// presence alone (the same well-known-path style as [`super::MANIFEST_MARKERS`]).
const TEST_CONFIG_MARKERS: &[&str] = &[
    "pytest.ini",
    "tox.ini",
    "jest.config.js",
    "jest.config.ts",
    "vitest.config.js",
    "vitest.config.ts",
    "phpunit.xml",
    "phpunit.xml.dist",
    ".rspec",
    "karma.conf.js",
];

/// CI configuration paths probed for a test invocation (relative to the repo
/// root). `.github/workflows` is a directory whose entries are each scanned; the
/// other two are single files. A documented well-known set mirroring the
/// enforcement-plane CI candidates.
const CI_DIR: &str = ".github/workflows";

const CI_FILES: &[&str] = &[".gitlab-ci.yml", ".circleci/config.yml"];

/// Documented test-command tokens. Their textual presence in a CI workflow or a
/// root context doc means the correct way to verify is discoverable. Matching is
/// mechanical token presence over a documented well-known set — the same
/// structural-scan discipline as the unused-import proxy, never a semantic
/// judgment of the command's adequacy. Lossy by contract (biased toward finding
/// reachability), and that is the conservative direction for an advisory probe.
const TEST_INVOCATION_MARKERS: &[&str] = &[
    "cargo test",
    "cargo nextest",
    "go test",
    "pytest",
    "npm test",
    "npm run test",
    "yarn test",
    "pnpm test",
    "make test",
    "phpunit",
    "rspec",
    "mvn test",
    "gradle test",
    "jest",
    "vitest",
    "ctest",
];

/// The package roots under `repo` for which the *correct way to verify a change*
/// is not statically discoverable before editing — the static "test/verification
/// reachability" leg of the audit (sibling to [`super::navigability_sites`]).
///
/// Verification is reachable for a package root when an agent could, by reading
/// the tree alone, find how to run its tests. Two discovery scopes satisfy it:
///
/// - **Repo-global** (short-circuits the whole probe to *empty*): a CI workflow
///   or a root context doc (README / AGENTS.md / CONTRIBUTING) that names a
///   documented test command ([`TEST_INVOCATION_MARKERS`]). A repo-wide
///   verification path is discoverable from one place, so no root is a site.
/// - **Per-package-local**: a [`TEST_DIRS`] directory, a `TEST_CONFIG_MARKERS`
///   file, a test-named source file, or an in-source `cfg(test)` module found by
///   a bounded walk *under* the root (skipping hidden / build-output dirs and
///   never following symlinks, like the rest of the family).
///
/// This is *reachability only* — presence of a verification entrypoint, never a
/// judgment of test adequacy or coverage (that semantic call belongs to a model,
/// not this probe; ZFC). Every signal is a mechanical filesystem / documented-
/// marker check. Born advisory like its siblings; the audit reports only the
/// count of unreachable roots.
pub fn verification_sites(repo: &Path) -> Result<Vec<PathBuf>, AuditError> {
    // A repo-global verification path is discoverable for every root at once.
    if has_repo_global_verification(repo)? {
        return Ok(Vec::new());
    }
    let mut sites = Vec::new();
    for root in package_roots(repo)? {
        if !has_local_verification(&root)? {
            sites.push(root);
        }
    }
    Ok(sites)
}

/// Count package roots with no statically discoverable verification entrypoint.
/// The count is exactly the length of [`verification_sites`].
pub(super) fn verification_reachability_item(
    repo: &Path,
    partition: &SubtreePartition,
) -> Result<Option<PunchItem>, AuditError> {
    let sites = verification_sites(repo)?;
    if sites.is_empty() {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "package roots without a reachable verification entrypoint".to_string(),
        kind: FindingKind::VerificationReachability,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(sites.len() as u64, "package roots"),
        plane: None,
        subtree: common_subtree(partition, sites.iter()),
    }))
}

/// Whether a repo-wide verification path is discoverable: a CI workflow or a root
/// context doc that names a documented test command. Either makes the correct way
/// to verify reachable for the whole repo at once.
fn has_repo_global_verification(repo: &Path) -> Result<bool, AuditError> {
    Ok(has_ci_test_step(repo)? || has_doc_test_command(repo)?)
}

/// Whether any CI config carries a documented test command. Scans every entry of
/// `.github/workflows/` plus the single-file CI configs ([`CI_FILES`]) for a
/// [`TEST_INVOCATION_MARKERS`] token. Missing CI files are simply absent signals,
/// and symlinked ones are never followed (the family's no-follow discipline).
fn has_ci_test_step(repo: &Path) -> Result<bool, AuditError> {
    let workflows = repo.join(CI_DIR);
    if workflows.is_dir() {
        for entry in read_dir(&workflows)? {
            let entry = entry.map_err(|source| io_err(&workflows, source))?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
            if file_type.is_file() && file_mentions_test_command(&path)? {
                return Ok(true);
            }
        }
    }
    for rel in CI_FILES {
        let path = repo.join(rel);
        // `symlink_metadata` does not follow symlinks; a symlinked CI config is
        // ignored, and a missing one is simply an absent signal.
        let is_regular = std::fs::symlink_metadata(&path)
            .map(|meta| meta.is_file())
            .unwrap_or(false);
        if is_regular && file_mentions_test_command(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a root context doc (README / AGENTS.md / CONTRIBUTING, case-insensitive)
/// names a documented test command. Only the repo-root level is scanned — a
/// documented verify step lives in the project's front-door docs.
fn has_doc_test_command(repo: &Path) -> Result<bool, AuditError> {
    for entry in read_dir(repo)? {
        let entry = entry.map_err(|source| io_err(repo, source))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        if file_type.is_file() && is_context_doc(&path) && file_mentions_test_command(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `path`'s name is a root context doc: `README*` / `CONTRIBUTING*`
/// (case-insensitive), `AGENTS.md`, or `CLAUDE.md` — the de-facto agent context
/// files where a project's test command is commonly documented.
fn is_context_doc(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower.starts_with("readme")
        || lower.starts_with("contributing")
        || lower == "agents.md"
        || lower == "claude.md"
}

/// Whether `path`'s (capped) text contains any documented test-command token. An
/// oversized file is treated as no signal rather than aborting the probe.
fn file_mentions_test_command(path: &Path) -> Result<bool, AuditError> {
    let Some(src) = read_source_capped(path)? else {
        return Ok(false);
    };
    Ok(TEST_INVOCATION_MARKERS.iter().any(|m| src.contains(m)))
}

/// Whether a verification entrypoint is reachable *within* `root` by a walk
/// bounded by [`super::SKIP_DIRS`], hidden-dir exclusions, and [`super::MAX_WALK_DEPTH`]: a
/// [`TEST_DIRS`] directory, a `TEST_CONFIG_MARKERS` file, a test-named source
/// file, or a `.rs` file with an in-source `cfg(test)` module. Short-circuits on
/// the first hit. Never follows symlinks — `.github` (hidden) is therefore
/// handled only by [`has_ci_test_step`].
fn has_local_verification(root: &Path) -> Result<bool, AuditError> {
    has_local_verification_bounded(root, 0)
}

fn has_local_verification_bounded(root: &Path, depth: usize) -> Result<bool, AuditError> {
    if depth >= MAX_WALK_DEPTH {
        return Ok(false);
    }
    for entry in read_dir(root)? {
        let entry = entry.map_err(|source| io_err(root, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| io_err(&path, source))?;
        if file_type.is_dir() {
            if TEST_DIRS.contains(&name.as_ref())
                || has_local_verification_bounded(&path, depth + 1)?
            {
                return Ok(true);
            }
        } else if file_type.is_file() {
            if TEST_CONFIG_MARKERS.contains(&name.as_ref()) || is_test_file(&name) {
                return Ok(true);
            }
            // The Rust unit-test convention lives in-source; a capped read avoids
            // a pathological file aborting the walk (oversized -> no signal).
            if is_rust_file(&path) {
                if let Some(src) = read_source_capped(&path)? {
                    if src.contains("cfg(test)") {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

/// Whether `name` follows a conventional test-file naming pattern across the
/// common ecosystems (Go `*_test.go`, Python `test_*`/`*_test.py`, JS/TS
/// `*.test.*`/`*.spec.*`, Ruby `*_spec.rb`, Java `*Test.java`/`*Tests.java`). A
/// documented well-known set; a name match is a discoverable entrypoint.
fn is_test_file(name: &str) -> bool {
    const JS_TEST_EXTS: &[&str] = &[".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"];
    name.ends_with("_test.go")
        || name.ends_with("_test.py")
        || (name.starts_with("test_") && name.ends_with(".py"))
        || name.ends_with("_spec.rb")
        || name.ends_with("Test.java")
        || name.ends_with("Tests.java")
        || JS_TEST_EXTS.iter().any(|ext| {
            has_infix_before_ext(name, ".test", ext) || has_infix_before_ext(name, ".spec", ext)
        })
}

/// Whether `name` is `<base><infix><ext>` with a non-empty base (e.g.
/// `button.test.ts` matches infix `.test`, ext `.ts`; bare `.test.ts` does not).
fn has_infix_before_ext(name: &str, infix: &str, ext: &str) -> bool {
    name.len() > infix.len() + ext.len()
        && name.ends_with(ext)
        && name[..name.len() - ext.len()].ends_with(infix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::tests::{implicit, tmp, workspace};
    use std::fs;

    #[test]
    fn verification_site_when_no_entrypoint_is_discoverable() {
        // A package root with source but no test dir, no test files, no test
        // config, no CI test step, and no documented command: the correct way to
        // verify a change is undiscoverable -> a site.
        let dir = tmp("verify-none");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(sites.contains(&dir), "root has no reachable verification");
        assert_eq!(sites.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_reachable_via_a_tests_directory() {
        let dir = tmp("verify-tests-dir");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("tests").join("it.rs"), "#[test]\nfn t() {}\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            !sites.contains(&dir),
            "a tests/ dir makes verification reachable"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_reachable_via_an_in_source_cfg_test_module() {
        // The Rust unit-test convention: tests live in a #[cfg(test)] module in
        // the same source file, with no tests/ dir. That is still discoverable.
        let dir = tmp("verify-cfg-test");
        fs::write(
            dir.join("lib.rs"),
            "pub fn f() {}\n#[cfg(test)]\nmod tests { #[test] fn t() {} }\n",
        )
        .unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            !sites.contains(&dir),
            "#[cfg(test)] makes verification reachable"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_reachable_via_a_test_named_source_file() {
        // Go/Python/JS conventions name test files; presence is reachability.
        let dir = tmp("verify-test-file");
        fs::write(dir.join("main.go"), "package main\n").unwrap();
        fs::write(dir.join("main_test.go"), "package main\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            !sites.contains(&dir),
            "a *_test.go file is a reachable entrypoint"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_reachable_via_a_test_config_file() {
        let dir = tmp("verify-config");
        fs::write(dir.join("app.py"), "x = 1\n").unwrap();
        fs::write(dir.join("pytest.ini"), "[pytest]\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            !sites.contains(&dir),
            "pytest.ini documents the verification path"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_reachable_via_a_ci_test_step() {
        // No local tests at all, but a CI workflow invokes the test runner: an
        // agent can discover the correct way to verify by reading the workflow.
        let dir = tmp("verify-ci");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let wf = dir.join(".github").join("workflows");
        fs::create_dir_all(&wf).unwrap();
        fs::write(
            wf.join("ci.yml"),
            "jobs:\n  t:\n    steps:\n      - run: cargo test\n",
        )
        .unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            sites.is_empty(),
            "a CI test step makes verification reachable repo-wide"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ci_probe_does_not_follow_a_symlinked_ci_config() {
        use std::os::unix::fs::symlink;
        // A CI config symlinked to an out-of-tree file with a test command must
        // not count: the probe family never follows symlinks.
        let base = tmp("verify-ci-symlink");
        let dir = base.join("repo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let outside = base.join("outside.yml");
        fs::write(&outside, "test:\n  script: cargo test\n").unwrap();
        symlink(&outside, dir.join(".gitlab-ci.yml")).unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            sites.contains(&dir),
            "a symlinked CI config must not make verification reachable"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn verification_reachable_via_a_documented_command() {
        // A documented test command in a root context doc is discoverable.
        let dir = tmp("verify-doc");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            dir.join("AGENTS.md"),
            "## Verify\nRun `make test` before committing.\n",
        )
        .unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            sites.is_empty(),
            "a documented command makes verification reachable"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_partial_across_member_crates() {
        // Monorepo: the root is covered by its own tests/ dir, member foo has a
        // local test file, member bar has nothing -> only bar is a site. No
        // repo-global CI/doc signal, so per-package reachability is what decides.
        let dir = tmp("verify-partial");
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("tests").join("it.rs"), "#[test]\nfn t() {}\n").unwrap();

        let foo = dir.join("crates").join("foo");
        fs::create_dir_all(foo.join("src")).unwrap();
        fs::write(foo.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(foo.join("src").join("lib.rs"), "#[cfg(test)]\nmod t {}\n").unwrap();

        let bar = dir.join("crates").join("bar");
        fs::create_dir_all(bar.join("src")).unwrap();
        fs::write(bar.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(bar.join("src").join("lib.rs"), "pub fn f() {}\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            sites.contains(&bar),
            "bar has no reachable verification -> a site"
        );
        assert!(!sites.contains(&foo), "foo's #[cfg(test)] is reachable");
        assert!(!sites.contains(&dir), "the root's tests/ dir is reachable");
        assert_eq!(sites.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_walk_skips_build_output_and_hidden_dirs() {
        // A test file buried in target/ or a hidden dir must NOT confer
        // reachability — those are not "the codebase".
        let dir = tmp("verify-skip");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let target = dir.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("gen_test.go"), "package gen\n").unwrap();
        let hidden = dir.join(".cache");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("x_test.go"), "package x\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            sites.contains(&dir),
            "tests under target/ or hidden dirs do not count"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn verification_walk_does_not_follow_symlinked_dirs() {
        // A test entrypoint reachable only by following a symlink out of the
        // package must NOT confer reachability — mirrors the symlink non-follow
        // invariant the navigability and size walks already guard.
        use std::os::unix::fs::symlink;
        let base = tmp("verify-symlink");
        let repo = base.join("repo");
        let outside = base.join("outside");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(outside.join("tests")).unwrap();
        fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(outside.join("tests").join("it.rs"), "#[test]\nfn t() {}\n").unwrap();
        symlink(&outside, repo.join("link")).unwrap();

        let sites = verification_sites(&repo).unwrap();
        assert!(
            sites.contains(&repo),
            "a tests/ dir reachable only through a symlink does not count"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn verification_root_reachable_only_via_a_member_crate_is_not_a_site() {
        // The root has no test path, CI, or doc of its own; reachability is
        // conferred solely by a member crate's #[cfg(test)] under crates/ (not a
        // SKIP_DIR). The probe is biased toward finding reachability, so neither
        // the root nor the member is a site. Locks that documented direction.
        let dir = tmp("verify-root-via-member");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let foo = dir.join("crates").join("foo");
        fs::create_dir_all(foo.join("src")).unwrap();
        fs::write(foo.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(foo.join("src").join("lib.rs"), "#[cfg(test)]\nmod t {}\n").unwrap();

        let sites = verification_sites(&dir).unwrap();
        assert!(
            !sites.contains(&dir),
            "root is reachable via the member crate's #[cfg(test)]"
        );
        assert!(!sites.contains(&foo), "foo's own #[cfg(test)] is reachable");
        assert!(sites.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_reachability_item_is_tier3_and_counts_sites() {
        let dir = tmp("verify-item");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = verification_reachability_item(&dir, &implicit(&dir))
            .unwrap()
            .expect("item");
        assert_eq!(item.kind, FindingKind::VerificationReachability);
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "package roots");
        assert_eq!(item.measured_cost.value, 1);
        assert!(item.plane.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_verification_item_when_all_roots_are_reachable() {
        let dir = tmp("verify-item-none");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("tests").join("it.rs"), "#[test]\nfn t() {}\n").unwrap();

        assert!(verification_reachability_item(&dir, &implicit(&dir))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verification_finding_attributes_to_its_subtree() {
        let (dir, partition) = workspace("attr-verify");
        // foo's #[cfg(test)] covers foo AND (via the recursive walk) the root;
        // bar alone has no reachable verification.
        fs::create_dir_all(dir.join("crates/foo/src")).unwrap();
        fs::write(
            dir.join("crates/foo/src/lib.rs"),
            "#[cfg(test)]\nmod t {}\n",
        )
        .unwrap();
        fs::write(dir.join("crates/bar/lib.rs"), "pub fn f() {}\n").unwrap();

        let item = verification_reachability_item(&dir, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(item.subtree.as_deref(), Some("crates/bar"));
        fs::remove_dir_all(&dir).ok();
    }
}
