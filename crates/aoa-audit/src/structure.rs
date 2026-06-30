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

/// Directory names that conventionally hold a package's tests
/// (`tests/` for Rust/Python, `__tests__/` for JS, `spec/` for Ruby/JS). A
/// documented well-known set — the same mechanical name match as
/// [`WORKSPACE_CONTAINER_DIRS`]; the presence of such a dir is a discoverable
/// verification entrypoint, not a quality judgment.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec"];

/// Test-runner configuration filenames. A directory carrying one documents how
/// the project's tests are run — a reachable verification entrypoint by file
/// presence alone (the same well-known-path style as [`MANIFEST_MARKERS`]).
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

/// Rule/invariant marker filenames whose presence means the project's *declared
/// conventions* are statically discoverable before an agent edits — a policy
/// file, an agent-context doc, a lint/format config, a pre-commit config, or a
/// CODEOWNERS. Matched case-insensitively, the mechanical equivalent of the
/// verification family's [`TEST_CONFIG_MARKERS`]: presence of a well-known
/// rule-file marker, never a judgment of the *content* of the rules (ZFC). The
/// `CONTRIBUTING*` doc and `.eslintrc*` family (many extensions) are matched by
/// prefix in [`is_invariant_file`] instead. README is deliberately *excluded*:
/// it is the navigability anchor ([`navigability_sites`]), a distinct construct.
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
/// the `.github/` and `docs/` placements, probed by filename like [`CI_FILES`].
const CODEOWNERS_PATHS: &[&str] = &[".github/CODEOWNERS", "docs/CODEOWNERS"];

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
    if let Some(item) = verification_reachability_item(repo)? {
        items.push(item);
    }
    if let Some(item) = invariant_discoverability_item(repo)? {
        items.push(item);
    }
    Ok(items)
}

/// The package roots under `repo` that lack a navigability anchor (README) —
/// the [`package_roots`] minus any that already have a README.
///
/// This is the per-site finding behind the navigability measure. The audit
/// reports only its *count* (a measured fact), but `aoa-migrate` consumes the
/// concrete sites so a migration fixes *exactly* what the audit measured.
pub fn navigability_sites(repo: &Path) -> Result<Vec<PathBuf>, AuditError> {
    let mut roots = package_roots(repo)?;
    roots.retain(|root| !has_readme(root));
    Ok(roots)
}

/// The package roots under `repo`: the repo root, every immediate child carrying
/// a build manifest, and workspace members nested one level inside a well-known
/// container dir (`crates/foo/`, `packages/bar/`; see [`WORKSPACE_CONTAINER_DIRS`]).
///
/// The single bounded package-root discovery shared by the structure probes that
/// key on package roots ([`navigability_sites`] and [`verification_sites`]) — one
/// walk, so the two cannot drift apart on what counts as a member.
///
/// Discovery is deliberately *bounded*, not a full-tree manifest sweep: an
/// unbounded walk would fold in trybuild test-fixture crates, `examples/`
/// sub-crates, and partially-vendored trees, inflating the count past the
/// construct it names ("workspace member crate") and — because `aoa-migrate`
/// *writes* READMEs into navigability sites — writing anchors into test
/// fixtures. The container-dir convention captures real members while excluding
/// those.
fn package_roots(repo: &Path) -> Result<Vec<PathBuf>, AuditError> {
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

/// The package roots under `repo` for which the *correct way to verify a change*
/// is not statically discoverable before editing — the static "test/verification
/// reachability" leg of the audit (sibling to [`navigability_sites`]).
///
/// Verification is reachable for a package root when an agent could, by reading
/// the tree alone, find how to run its tests. Two discovery scopes satisfy it:
///
/// - **Repo-global** (short-circuits the whole probe to *empty*): a CI workflow
///   or a root context doc (README / AGENTS.md / CONTRIBUTING) that names a
///   documented test command ([`TEST_INVOCATION_MARKERS`]). A repo-wide
///   verification path is discoverable from one place, so no root is a site.
/// - **Per-package-local**: a [`TEST_DIRS`] directory, a [`TEST_CONFIG_MARKERS`]
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
fn verification_reachability_item(repo: &Path) -> Result<Option<PunchItem>, AuditError> {
    let missing = verification_sites(repo)?.len();
    if missing == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "package roots without a reachable verification entrypoint".to_string(),
        kind: FindingKind::VerificationReachability,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(missing as u64, "package roots"),
        plane: None,
    }))
}

/// The package roots under `repo` for which the project's *declared rules /
/// invariants* are not statically discoverable before editing — the static
/// "invariant reachability" leg of the audit (sibling to [`verification_sites`]).
///
/// Rules are discoverable for a package root when an agent could, by reading the
/// tree alone, find the project's declared conventions before touching code. Two
/// discovery scopes satisfy it, mirroring [`verification_sites`]:
///
/// - **Repo-global** (short-circuits the whole probe to *empty*): a `.aoa` policy
///   dir, a non-root CODEOWNERS ([`CODEOWNERS_PATHS`]), or any root-level
///   [`INVARIANT_FILE_MARKERS`] file. A repo-wide rule source is discoverable
///   from one front-door place, so no root is a site.
/// - **Per-package-local**: an [`INVARIANT_FILE_MARKERS`] file found by a bounded
///   walk *under* the root (skipping build-output and hidden *directories* and
///   never following symlinks). Unlike [`has_local_verification`], a leading-dot
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
fn invariant_discoverability_item(repo: &Path) -> Result<Option<PunchItem>, AuditError> {
    let missing = invariant_sites(repo)?.len();
    if missing == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "package roots without discoverable rules/invariants".to_string(),
        kind: FindingKind::InvariantDiscoverability,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(missing as u64, "package roots"),
        plane: None,
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
/// fixed-name one-level probe that cannot escape the tree (like [`has_manifest`]).
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
/// by [`SKIP_DIRS`] and hidden-*directory* exclusions (depth-unlimited within
/// those bounds). Short-circuits on the first hit. Never follows symlinks.
///
/// Divergence from [`has_local_verification`] by design: a leading-dot *file* is
/// NOT skipped here, because rule files are overwhelmingly dotfiles
/// (`.editorconfig`, `.eslintrc.json`, `.pre-commit-config.yaml`); only hidden
/// *directories* are pruned. The hidden `.aoa` dir is therefore a repo-global
/// signal only ([`has_repo_global_invariants`]), never found by this walk.
fn has_local_invariants(root: &Path) -> Result<bool, AuditError> {
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
            if has_local_invariants(&path)? {
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

/// Whether a repo-wide verification path is discoverable: a CI workflow or a root
/// context doc that names a documented test command. Either makes the correct way
/// to verify reachable for the whole repo at once.
fn has_repo_global_verification(repo: &Path) -> Result<bool, AuditError> {
    Ok(has_ci_test_step(repo)? || has_doc_test_command(repo)?)
}

/// Whether any CI config carries a documented test command. Scans every entry of
/// `.github/workflows/` plus the single-file CI configs ([`CI_FILES`]) for a
/// [`TEST_INVOCATION_MARKERS`] token. Missing CI files are simply absent signals.
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
        if path.is_file() && file_mentions_test_command(&path)? {
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
/// bounded by [`SKIP_DIRS`] and hidden-dir exclusions (depth-unlimited within
/// those bounds): a [`TEST_DIRS`] directory, a [`TEST_CONFIG_MARKERS`] file, a
/// test-named source file, or a `.rs` file with an in-source `cfg(test)` module.
/// Short-circuits on the first hit. Never follows symlinks — `.github` (hidden)
/// is therefore handled only by [`has_ci_test_step`].
fn has_local_verification(root: &Path) -> Result<bool, AuditError> {
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
            if TEST_DIRS.contains(&name.as_ref()) || has_local_verification(&path)? {
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

    // --- verification (test) reachability ---

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

        let item = verification_reachability_item(&dir).unwrap().expect("item");
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

        assert!(verification_reachability_item(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    // --- invariant (rules) discoverability ---

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

        let item = invariant_discoverability_item(&dir).unwrap().expect("item");
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

        assert!(invariant_discoverability_item(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
