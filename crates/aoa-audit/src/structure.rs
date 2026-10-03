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
//! review-acceptance, the R9c discipline in `aoa-construct`) promotes it. We
//! therefore report only neutral, measured counts — never an opinion-bearing
//! "deficiency" — so the audit *verifies* a pre-registered spec rather than
//! *defining* one (anti-Goodhart; see `docs/r0_runbook.md`).
//!
//! # Factory agent-readiness pillar disposition (aoa-d6t.24)
//!
//! Factory's agent-readiness model (docs.factory.ai/web/agent-readiness)
//! defines nine pillars. Each was evaluated as a *hypothesis* for a
//! trace-testable structure probe — never adopted as a checkbox. Disposition,
//! one row per pillar:
//!
//! | Pillar | Disposition |
//! |---|---|
//! | Style / validation | **Covered**: [`invariant_sites`] (lint/format/policy discoverability, aoa-d6t.21) plus the enforcement-plane probes (pre-commit / CI presence). |
//! | Build system | **Probe**: [`declarations::build_determinism_item`] — dependency-pinning lockfile existence (`declarations::BUILD_DETERMINISM_MARKERS`). The "documented build command" sub-signal was DROPPED: a build-command token scan of front-door docs cannot be distinguished from the test-command scan [`verification_sites`] already performs without a semantic judgment of which command is "the build". |
//! | Testing | **Covered**: [`verification_sites`] (aoa-d6t.20). |
//! | Documentation | **Covered**: [`navigability_sites`] (README anchors) plus [`invariant_sites`]' agent-context / CONTRIBUTING markers. |
//! | Dev environment | **Probe**: [`declarations::dev_environment_item`] — reproducible-environment declaration existence (`declarations::DEV_ENVIRONMENT_MARKERS`). |
//! | Debugging / observability | **Excluded**: structured-logging/observability configuration is code-level and ecosystem-specific (a `tracing` subscriber in Rust, a logger setup in Go/JS are *source*, not fixed well-known filenames). The only fixed-filename conventions (`logback.xml`, `log4j2.xml`) are single-ecosystem and would bias the measure; anything broader needs per-ecosystem manifest parsing or token heuristics — forbidden (ZFC), so the pillar is dropped rather than half-built. |
//! | Security | **Split**: the write-safety leg is covered by aoa-d6t.16 (`generated_artifact_protection_absence`, `write_safety_zone_absence`; merged pending on `wave-d6t16-x0a1-review`) — deliberately not recreated here. The branch-protection / security-posture leg is **excluded**: branch protection lives in forge settings, not the checkout, so a read-only tree probe cannot observe it, and an agent trace never touches it mid-edit. |
//! | Task discovery | **Probe**: [`declarations::task_discovery_item`] — issue-template / in-repo-tracker surface existence (`declarations::TASK_DISCOVERY_SURFACES`). |
//! | Product / experimentation | **Excluded**: analytics/experimentation instrumentation is a product-layer semantic property with no fixed-filename convention and no plausible path from its presence to structural facts in coding-agent traces. |

mod declarations;
mod generated;
mod invariants;
mod size_outliers;
mod unused_imports;
mod verification;

pub use invariants::invariant_sites;
use size_outliers::{module_size_outlier_item, module_size_outliers};
pub use verification::verification_sites;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use aoa_metrics::SubtreePartition;

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
    "build.gradle.kts",
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

/// Recursion-depth ceiling for recursive filesystem walks. Real repos nest a few
/// dozen levels at most; the cap is defense-in-depth so a pathologically (or
/// maliciously) deep tree degrades to "no signal found below here" rather than
/// overflowing the stack.
const MAX_WALK_DEPTH: usize = 64;

/// Minimum number of source files required before the module-size measure is
/// meaningful: a median computed from a handful of files cannot self-calibrate,
/// so below this the check abstains (emits nothing) rather than assert an
/// outlier from noise.
const MIN_FILES_FOR_MEDIAN: usize = 5;

/// One code-structure probe, named once and rendered into both registers.
///
/// The audit publishes each probe twice — as a punch item for a human punch list
/// and as a count for the external-outcome corpus — and CLAUDE.md requires those
/// to be dual registers of one result, not two implementations. A `Probe` is that
/// one result: [`structure_items`] and [`structure_measurements`] both map over
/// [`PROBES`], so a probe cannot be added to one register and forgotten in the
/// other (the old failure mode, which read downstream as "no data" rather than as
/// a bug).
struct Probe {
    /// The finding this probe reports under, in both registers.
    kind: FindingKind,
    /// The corpus count, or `None` for a punch-list-only probe — one that backs
    /// no external-outcome gating candidate and so is deliberately absent from
    /// [`structure_measurements`]. The omission is structural, not prose: the
    /// probe declares it here.
    measure: Option<MeasureFn>,
    /// The punch item, absent when the probe abstains (a clean repo, or one it
    /// cannot assess).
    item: ItemFn,
}

/// A probe's corpus register: the repo and the module-size multiplier in, one
/// [`StructureMeasure`] out.
type MeasureFn = fn(&Path, f64) -> Result<StructureMeasure, AuditError>;

/// A probe's punch-list register: the same inputs plus the subtree partition that
/// scopes path-carrying findings.
type ItemFn = fn(&Path, f64, &SubtreePartition) -> Result<Option<PunchItem>, AuditError>;

/// Every code-structure probe, in punch-list order. The single list both public
/// entry points read; see [`Probe`].
const PROBES: &[Probe] = &[
    Probe {
        kind: FindingKind::NavigabilityAnchor,
        measure: Some(|repo, _k| {
            Ok(StructureMeasure::Measured(
                navigability_sites(repo)?.len() as u64
            ))
        }),
        item: |repo, _k, partition| navigability_anchor_item(repo, partition),
    },
    Probe {
        kind: FindingKind::ModuleSizeOutlier,
        measure: Some(|repo, k| {
            Ok(
                module_size_outliers(repo, k)?.map_or(StructureMeasure::Unmeasurable, |outliers| {
                    StructureMeasure::Measured(outliers.count())
                }),
            )
        }),
        item: module_size_outlier_item,
    },
    Probe {
        kind: FindingKind::UnusedImportProxy,
        measure: Some(|repo, _k| unused_imports::unused_import_measure(repo)),
        item: |repo, _k, partition| unused_imports::unused_import_proxy_item(repo, partition),
    },
    Probe {
        kind: FindingKind::VerificationReachability,
        measure: None,
        item: |repo, _k, partition| verification::verification_reachability_item(repo, partition),
    },
    Probe {
        kind: FindingKind::InvariantDiscoverability,
        measure: None,
        item: |repo, _k, partition| invariants::invariant_discoverability_item(repo, partition),
    },
    Probe {
        kind: FindingKind::BuildDeterminism,
        measure: Some(|repo, _k| {
            Ok(StructureMeasure::Measured(
                declarations::build_determinism_absent_count(repo),
            ))
        }),
        item: |repo, _k, _partition| Ok(declarations::build_determinism_item(repo)),
    },
    Probe {
        kind: FindingKind::DevEnvironmentDeclaration,
        measure: Some(|repo, _k| {
            Ok(StructureMeasure::Measured(
                declarations::dev_environment_absent_count(repo),
            ))
        }),
        item: |repo, _k, _partition| Ok(declarations::dev_environment_item(repo)),
    },
    Probe {
        kind: FindingKind::TaskDiscoverySurface,
        measure: Some(|repo, _k| {
            Ok(StructureMeasure::Measured(
                declarations::task_discovery_absent_count(repo),
            ))
        }),
        item: |repo, _k, _partition| Ok(declarations::task_discovery_item(repo)),
    },
    Probe {
        kind: FindingKind::GeneratedArtifactProtection,
        measure: Some(|repo, _k| {
            Ok(StructureMeasure::Measured(
                declarations::generated_artifact_protection_absent_count(repo)?,
            ))
        }),
        item: |repo, _k, _partition| declarations::generated_artifact_protection_item(repo),
    },
    Probe {
        kind: FindingKind::WriteSafetyZone,
        measure: Some(|repo, _k| {
            Ok(StructureMeasure::Measured(
                declarations::write_boundary_absent_count(repo),
            ))
        }),
        item: |repo, _k, _partition| Ok(declarations::write_safety_zone_item(repo)),
    },
];

/// Run the code-structure audit family over `repo`, returning measured-fact
/// punch items (each born [`Tier::Tier3`]). `size_outlier_k` is the caller's
/// documented multiplier for the module-size measure; `partition` scopes
/// path-carrying findings to their workspace subtree (see [`common_subtree`]).
pub(crate) fn structure_items(
    repo: &Path,
    size_outlier_k: f64,
    partition: &SubtreePartition,
) -> Result<Vec<PunchItem>, AuditError> {
    let mut items = Vec::new();
    for probe in PROBES {
        if let Some(item) = (probe.item)(repo, size_outlier_k, partition)? {
            items.push(item);
        }
    }
    Ok(items)
}

/// A structure measure's outcome over a repo.
///
/// The punch-list `Option<PunchItem>` encoding is lossy for correlation work: a
/// probe emits nothing both when it *measured zero* (the repo is clean) and when
/// it *could not measure* (e.g. too few files to form a median). Collapsing those
/// to "absent" suits a to-do list but silently stops a measure from ever varying
/// in an external-outcome corpus (aoa-3a7): a clean repo would never supply the
/// `(0, y)` anchor a correlation needs. `StructureMeasure` keeps the two apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureMeasure {
    /// The probe ran and counted `n` occurrences of the bad thing (`0` = clean).
    Measured(u64),
    /// The probe could not assess this repo, so there is no observation.
    Unmeasurable,
}

/// Measure the code-structure family over `repo` as counts, distinguishing a
/// measured zero from an inability to measure — the external-outcome-corpus view
/// of the same probes [`structure_items`] renders as a punch list (see
/// [`StructureMeasure`]). `size_outlier_k` is the audit's module-size multiplier.
///
/// Only the measures that back an external-outcome gating candidate are reported:
/// a punch-list-only probe declares itself by carrying no measure at all.
pub fn structure_measurements(
    repo: &Path,
    size_outlier_k: f64,
) -> Result<BTreeMap<FindingKind, StructureMeasure>, AuditError> {
    let mut m = BTreeMap::new();
    for probe in PROBES {
        if let Some(measure) = probe.measure {
            m.insert(probe.kind, measure(repo, size_outlier_k)?);
        }
    }
    Ok(m)
}

/// The one workspace subtree every path in `paths` attributes to, or `None`.
///
/// `None` when the partition is not meaningful (a single-member repo, where a
/// subtree label adds no signal — the same [`SubtreePartition::is_partitioned`]
/// gate the per-subtree metric rows apply), when any path falls outside every
/// member (e.g. the repo root itself in a workspace whose members are all
/// nested), or when the paths span more than one member. Attribution is
/// unanimous or absent — never a majority guess.
fn common_subtree<'p>(
    partition: &SubtreePartition,
    paths: impl Iterator<Item = &'p PathBuf>,
) -> Option<String> {
    if !partition.is_partitioned() {
        return None;
    }
    let mut subtrees = paths.map(|p| partition.attribute(&p.to_string_lossy()));
    let first = subtrees.next()??;
    subtrees
        .all(|s| s == Some(first))
        .then(|| first.to_string())
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

pub fn package_roots(repo: &Path) -> Result<Vec<PathBuf>, AuditError> {
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
    let mut seen: BTreeSet<PathBuf> = roots.iter().cloned().collect();
    for member in aoa_metrics::declared_member_dirs(repo).unwrap_or_default() {
        if let Some(root) = declared_member_root(repo, &member) {
            if seen.insert(root.clone()) {
                roots.push(root);
            }
        }
    }
    Ok(roots)
}

fn declared_member_root(repo: &Path, member: &str) -> Option<PathBuf> {
    let mut root = repo.to_path_buf();
    for component in Path::new(member).components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        if name.starts_with('.') || SKIP_DIRS.contains(&name) {
            return None;
        }
        root.push(name);
        if !std::fs::symlink_metadata(&root).is_ok_and(|meta| meta.is_dir()) {
            return None;
        }
    }
    Some(root)
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
fn navigability_anchor_item(
    repo: &Path,
    partition: &SubtreePartition,
) -> Result<Option<PunchItem>, AuditError> {
    let sites = navigability_sites(repo)?;
    if sites.is_empty() {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "package roots without a navigability anchor (README)".to_string(),
        kind: FindingKind::NavigabilityAnchor,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(sites.len() as u64, "package roots"),
        plane: None,
        subtree: common_subtree(partition, sites.iter()),
        size_outliers: None,
    }))
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

/// Whether `path` is a Rust source file.
fn is_rust_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("rs")
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

/// Recursively collect source-file paths and their line counts under `dir`,
/// skipping hidden and build-output directories and never following symlinks
/// (matching aoa-scip-graph's best-effort walk). The path rides along so an
/// outlier can be attributed to its workspace subtree. An oversized single
/// file is skipped, not fatal; a genuine read error propagates.
fn collect_source_line_counts(
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
            collect_source_line_counts(&path, out, depth + 1)?;
        } else if file_type.is_file() && is_source_file(&path) {
            // `None` is an oversized file: skipped, not fatal (the scan is a
            // lossy structural proxy by contract). A genuine read error
            // propagates.
            if let Some(n) = count_lines(&path)? {
                out.push((path, n));
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
    use std::collections::BTreeSet;
    use std::fs;

    pub(super) fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aoa-structure-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The single-subtree partition most fixtures need (no workspace manifest):
    /// attribution is gated off, so items carry `subtree: None`.
    pub(super) fn implicit(dir: &Path) -> SubtreePartition {
        SubtreePartition::implicit_root(dir)
    }

    fn create_deep_tree(root: &Path) -> PathBuf {
        (0..=MAX_WALK_DEPTH).fold(root.to_path_buf(), |dir, depth| {
            let nested = dir.join(format!("level-{depth}"));
            fs::create_dir(&nested).unwrap();
            nested
        })
    }

    /// Both registers key off [`Probe::kind`], so two probes sharing a kind would
    /// silently collide in the corpus map.
    #[test]
    fn every_probe_owns_exactly_one_finding_kind() {
        let kinds: BTreeSet<FindingKind> = PROBES.iter().map(|p| p.kind).collect();
        assert_eq!(kinds.len(), PROBES.len(), "duplicate probe kind in PROBES");
    }

    /// The corpus register reports exactly the measure-bearing probes — nothing a
    /// probe did not declare, and nothing silently dropped.
    #[test]
    fn structure_measurements_reports_every_measure_bearing_probe() {
        let dir = tmp("measure-register");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let reported: BTreeSet<FindingKind> = structure_measurements(&dir, 4.0)
            .unwrap()
            .into_keys()
            .collect();
        let declared: BTreeSet<FindingKind> = PROBES
            .iter()
            .filter(|p| p.measure.is_some())
            .map(|p| p.kind)
            .collect();
        assert_eq!(reported, declared);
        // The punch-list-only probes stay out of the corpus view.
        assert!(!reported.contains(&FindingKind::VerificationReachability));
        assert!(!reported.contains(&FindingKind::InvariantDiscoverability));
        fs::remove_dir_all(&dir).ok();
    }

    /// Every punch item a probe emits is attributed to that probe's own kind, so
    /// the punch list and the corpus counts cannot disagree about which finding a
    /// probe reports.
    #[test]
    fn each_probe_emits_items_under_its_own_kind() {
        let dir = tmp("item-register");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        let partition = implicit(&dir);
        for probe in PROBES {
            if let Some(item) = (probe.item)(&dir, 4.0, &partition).unwrap() {
                assert_eq!(item.kind, probe.kind);
            }
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn structure_measurements_records_a_measured_zero_for_a_clean_repo() {
        let dir = tmp("measure-clean");
        // A pinned build (Cargo.lock present) is a real measured 0 — the (0, y)
        // corpus anchor the punch list drops (aoa-3a7).
        fs::write(dir.join("Cargo.lock"), "").unwrap();
        let m = structure_measurements(&dir, 4.0).unwrap();
        assert_eq!(
            m[&FindingKind::BuildDeterminism],
            StructureMeasure::Measured(0)
        );
        // The punch-list behavior is preserved: no item for a clean measure.
        assert!(declarations::build_determinism_item(&dir).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn structure_measurements_records_one_for_an_absent_marker() {
        let dir = tmp("measure-absent");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap(); // no lockfile
        let m = structure_measurements(&dir, 4.0).unwrap();
        assert_eq!(
            m[&FindingKind::BuildDeterminism],
            StructureMeasure::Measured(1)
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn structure_measurements_marks_module_size_unmeasurable_below_the_median_floor() {
        let dir = tmp("measure-tiny");
        fs::write(dir.join("a.py"), "x = 1\n").unwrap(); // 1 file < MIN_FILES_FOR_MEDIAN
        let m = structure_measurements(&dir, 4.0).unwrap();
        assert_eq!(
            m[&FindingKind::ModuleSizeOutlier],
            StructureMeasure::Unmeasurable
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn structure_measurements_marks_unused_imports_unmeasurable_on_a_non_rust_repo() {
        let dir = tmp("measure-nonrust");
        fs::write(dir.join("app.py"), "import os\n").unwrap();
        let m = structure_measurements(&dir, 4.0).unwrap();
        assert_eq!(
            m[&FindingKind::UnusedImportProxy],
            StructureMeasure::Unmeasurable
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn structure_measurements_ignore_sources_below_the_walk_depth_limit() {
        let dir = tmp("measure-depth-limit");
        fs::write(dir.join("shallow.rs"), "fn shallow() {}\n").unwrap();

        let deep = create_deep_tree(&dir);
        for i in 0..5 {
            fs::write(deep.join(format!("small-{i}.rs")), "fn small() {}\n").unwrap();
        }
        fs::write(
            deep.join("huge.rs"),
            format!("use std::path::Path;\n{}", "fn huge() {}\n".repeat(200)),
        )
        .unwrap();

        let measurements = structure_measurements(&dir, 4.0).unwrap();
        assert_eq!(
            (
                measurements[&FindingKind::ModuleSizeOutlier],
                measurements[&FindingKind::UnusedImportProxy],
            ),
            (
                StructureMeasure::Unmeasurable,
                StructureMeasure::Measured(0),
            ),
            "source files below the recursion cap must not affect either measure",
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// A two-member Cargo-workspace fixture (`crates/foo`, `crates/bar`) whose
    /// discovered partition is meaningful, for the attribution tests.
    pub(super) fn workspace(name: &str) -> (PathBuf, SubtreePartition) {
        let dir = tmp(name);
        fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/foo\", \"crates/bar\"]\n",
        )
        .unwrap();
        for member in ["foo", "bar"] {
            let root = dir.join("crates").join(member);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        }
        let partition = aoa_metrics::discover_partition(&dir).unwrap();
        assert!(partition.is_partitioned(), "fixture must be a workspace");
        (dir, partition)
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

        let item = navigability_anchor_item(&dir, &implicit(&dir))
            .unwrap()
            .expect("item");
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "package roots");
        assert_eq!(item.measured_cost.value, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_navigability_item_when_root_has_readme() {
        let dir = tmp("nav-present");
        fs::write(dir.join("README.md"), "# repo\n").unwrap();

        assert!(navigability_anchor_item(&dir, &implicit(&dir))
            .unwrap()
            .is_none());
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

        let item = navigability_anchor_item(&dir, &implicit(&dir))
            .unwrap()
            .expect("item");
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

        let item = module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .expect("item");
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
        assert!(module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_outlier_abstains_below_the_minimum_file_count() {
        let dir = tmp("size-too-few");
        // Two files, one much larger: too few to self-calibrate a median.
        fs::write(dir.join("a.rs"), "x\n").unwrap();
        fs::write(dir.join("b.rs"), "x\n".repeat(500)).unwrap();
        assert!(module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .is_none());
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
        assert!(module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .is_none());
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
        collect_source_line_counts(&dir, &mut counts, 0).unwrap();
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
        collect_source_line_counts(&dir, &mut counts, 0).unwrap();
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

        assert!(module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .is_none());
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
        collect_source_line_counts(&repo, &mut counts, 0).unwrap();
        assert_eq!(counts.len(), 6, "symlinked dir must not be traversed");
        fs::remove_dir_all(&base).ok();
    }

    // --- unused-import syntactic proxy ---

    // --- verification (test) reachability ---

    // --- invariant (rules) discoverability ---

    // --- build determinism (Factory build-system pillar) ---
    //
    // These probes read no file contents (fixed-path existence only), so the
    // non-UTF-8 quadrant the content-scanning siblings need is structurally
    // inapplicable here.

    // --- generated-artifact protection (R6) ---

    // --- dev-environment declaration (Factory dev-environment pillar) ---

    // --- task-discovery surface (Factory task-discovery pillar) ---

    // --- write-safety zone (R5) ---

    #[test]
    fn structure_items_include_the_factory_pillar_probes() {
        // Integration: a bare repo yields all three pillar findings via the
        // family entry point.
        let dir = tmp("factory-integration");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let items = structure_items(&dir, 4.0, &implicit(&dir)).unwrap();
        let kinds: Vec<FindingKind> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&FindingKind::BuildDeterminism));
        assert!(kinds.contains(&FindingKind::DevEnvironmentDeclaration));
        assert!(kinds.contains(&FindingKind::TaskDiscoverySurface));
        assert!(kinds.contains(&FindingKind::GeneratedArtifactProtection));
        assert!(kinds.contains(&FindingKind::WriteSafetyZone));
        fs::remove_dir_all(&dir).ok();
    }

    // --- subtree attribution (aoa-d6t.31) ---

    #[test]
    fn navigability_finding_attributes_to_its_single_subtree() {
        let (dir, partition) = workspace("attr-nav-single");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        fs::write(dir.join("crates/bar/README.md"), "# bar\n").unwrap();
        // Only crates/foo lacks a README: the finding is scoped to that member.

        let item = navigability_anchor_item(&dir, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(item.subtree.as_deref(), Some("crates/foo"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_finding_spanning_members_is_repo_wide() {
        let (dir, partition) = workspace("attr-nav-span");
        fs::write(dir.join("README.md"), "# root\n").unwrap();
        // Both members lack a README: no single subtree owns the finding.

        let item = navigability_anchor_item(&dir, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 2);
        assert!(
            item.subtree.is_none(),
            "spanning finding must stay repo-wide"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigability_finding_touching_the_root_is_repo_wide() {
        let (dir, partition) = workspace("attr-nav-root");
        fs::write(dir.join("crates/bar/README.md"), "# bar\n").unwrap();
        // The repo root (outside every member) and crates/foo both lack a
        // README: a path outside all members blocks attribution.

        let item = navigability_anchor_item(&dir, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 2);
        assert!(item.subtree.is_none(), "root site is outside every member");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_outlier_finding_attributes_to_its_subtree() {
        let (dir, partition) = workspace("attr-size");
        for i in 0..6 {
            fs::write(dir.join(format!("crates/bar/m{i}.rs")), "x\n".repeat(10)).unwrap();
        }
        fs::write(dir.join("crates/foo/huge.rs"), "x\n".repeat(200)).unwrap();

        let item = module_size_outlier_item(&dir, 4.0, &partition)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(item.subtree.as_deref(), Some("crates/foo"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn attribution_is_gated_off_for_a_single_subtree_repo() {
        // An implicit-root partition attributes every path to `.`, which adds
        // no signal — the is_partitioned gate keeps the field None (and thus
        // off the wire) for non-workspace repos.
        let dir = tmp("attr-implicit");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = navigability_anchor_item(&dir, &implicit(&dir))
            .unwrap()
            .expect("item");
        assert!(item.subtree.is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
