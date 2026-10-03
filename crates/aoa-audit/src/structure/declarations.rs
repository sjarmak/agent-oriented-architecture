//! The declaration probes: five fixed-marker existence checks.
//!
//! Each asks the same shape of question — does this repository declare the
//! convention at all — over a documented list of well-known paths, and each
//! reports the count of marker families that are ABSENT. They share a module
//! because they share that one reason to change: a new convention is a new row
//! in one of the const lists below, never new logic.
//!
//! Nothing here reads a file to judge its contents; the closest any comes is
//! [`declares_linguist_generated`]'s literal substring match, which is a
//! presence test spelled over bytes rather than over a filename.

use std::path::Path;

use super::read_source_capped;
use crate::error::AuditError;
use crate::punch::{FindingKind, MeasuredCost, PunchItem};
use crate::tier::Tier;

/// Dependency-pinning lockfile names probed at the repo root. Any one existing
/// declares deterministic build inputs (the Factory build-system pillar's
/// trace-testable fact). A documented well-known set in the
/// [`super::MANIFEST_MARKERS`] style; membership is by exact-name existence alone —
/// lossy by contract (a repo pinning some other way reads as absent), which is
/// the conservative direction for an advisory measure.
const BUILD_DETERMINISM_MARKERS: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "go.sum",
    "Gemfile.lock",
    "composer.lock",
    "mix.lock",
    "flake.lock",
    "gradle.lockfile",
    "packages.lock.json",
];

/// Reproducible dev-environment declarations probed at fixed paths relative to
/// the repo root: a devcontainer, a nix flake/shell, or a toolchain/runtime
/// version pin. Any one existing means an agent (or CI) can reconstruct the
/// intended environment from the tree alone. `Dockerfile` is deliberately
/// EXCLUDED: a Dockerfile is a deployment artifact as often as a dev
/// environment, and telling those apart is a semantic classification outside
/// this probe's mechanical contract (ZFC).
const DEV_ENVIRONMENT_MARKERS: &[&str] = &[
    ".devcontainer/devcontainer.json",
    ".devcontainer.json",
    "flake.nix",
    "shell.nix",
    "rust-toolchain.toml",
    "rust-toolchain",
    ".tool-versions",
    "mise.toml",
    ".mise.toml",
    ".nvmrc",
    ".node-version",
    ".python-version",
    ".ruby-version",
];

/// Task-discovery surfaces probed at fixed paths relative to the repo root: the
/// documented issue-template locations (GitHub's `.github/ISSUE_TEMPLATE` dir or
/// single-file forms, GitLab's `.gitlab/issue_templates`) and the in-repo
/// `.beads` issue tracker (the toolkit-ecosystem convention, same precedent as
/// the `.aoa` dir in `INVARIANT_DIR`). Any one existing means structured work
/// items are discoverable from the tree. Exact-name matches only — lossy by
/// contract, like the sibling marker sets.
const TASK_DISCOVERY_SURFACES: &[&str] = &[
    ".github/ISSUE_TEMPLATE",
    ".github/ISSUE_TEMPLATE.md",
    "ISSUE_TEMPLATE.md",
    "docs/ISSUE_TEMPLATE.md",
    ".gitlab/issue_templates",
    ".beads",
];

/// The attribute token a `.gitattributes` entry uses to mark a path as a
/// generated artifact (so an agent does not edit derived state). A documented,
/// well-known marker — the same fixed-name style as [`super::MANIFEST_MARKERS`] — and
/// the only generated-artifact-protection convention this probe recognizes. The
/// probe asks one purely structural question (does the repo *declare* this
/// convention at all), never which files are generated — that classification is
/// a semantic judgment outside the audit's mechanical contract.
const LINGUIST_GENERATED_ATTR: &str = "linguist-generated";

/// The well-known write-boundary declaration surfaces the write-safety probe
/// looks for, grouped by *kind*. Each inner slice is one kind, present if any of
/// its candidate paths exists; the measure counts the kinds that are absent. A
/// repository that declares such a surface is funnelling writes through a narrow,
/// auditable boundary (the report's R5 "mutation gateway / ownership metadata"
/// signal). The set is a documented well-known-name list, like [`super::SKIP_DIRS`];
/// membership is by path existence alone — no parsing, no heuristic.
///
/// CODEOWNERS is probed at each of GitHub's three documented locations (repo
/// root, `.github/`, `docs/`); `.aoa/write-policy.toml` is the toolkit's own
/// declared safe-write-zone surface (the same `.aoa/` namespace the enforcement
/// planes probe).
const WRITE_BOUNDARY_SURFACES: &[(&str, &[&str])] = &[
    (
        "CODEOWNERS",
        &["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"],
    ),
    (
        "AOA safe-write-zone policy (.aoa/write-policy.toml)",
        &[".aoa/write-policy.toml"],
    ),
];

/// Whether the repo declares deterministic build inputs: any well-known
/// dependency lockfile ([`BUILD_DETERMINISM_MARKERS`]) at the repo root. The
/// measure is the count of this single marker family that is ABSENT (0 when any
/// lockfile exists, 1 when none does) — the Factory build-system pillar reduced
/// to its one mechanically checkable fact. Fixed-path existence only (`exists()`
/// follows a symlinked lockfile, like [`super::has_manifest`]); no file content is ever
/// read, so the probe is infallible. A repo with no package manager at all reads
/// the same as one that has not pinned — a neutral measured fact either way,
/// exactly like the sibling absence probes. Born [`Tier::Tier3`].
pub(super) fn build_determinism_item(repo: &Path) -> Option<PunchItem> {
    absence_item(
        repo,
        BUILD_DETERMINISM_MARKERS,
        "repository has no dependency-pinning lockfile (build determinism undeclared)",
        FindingKind::BuildDeterminism,
        "build-determinism markers absent",
    )
}

/// Whether the repo declares a reproducible dev environment: any well-known
/// devcontainer / nix / toolchain-pin path ([`DEV_ENVIRONMENT_MARKERS`]). The
/// measure is the count of this single declaration family that is ABSENT — the
/// Factory dev-environment pillar's mechanically checkable fact. Fixed-path
/// existence only; infallible; born [`Tier::Tier3`].
pub(super) fn dev_environment_item(repo: &Path) -> Option<PunchItem> {
    absence_item(
        repo,
        DEV_ENVIRONMENT_MARKERS,
        "repository has no reproducible dev-environment declaration \
         (devcontainer / flake / toolchain pin)",
        FindingKind::DevEnvironmentDeclaration,
        "dev-environment declarations absent",
    )
}

/// Whether the repo exposes a task-discovery surface: any well-known
/// issue-template path or in-repo tracker ([`TASK_DISCOVERY_SURFACES`]). The
/// measure is the count of this single surface family that is ABSENT — the
/// Factory task-discovery pillar's mechanically checkable fact. Fixed-path
/// existence only (a path may be a file or a directory; `exists()` accepts
/// both); infallible; born [`Tier::Tier3`].
pub(super) fn task_discovery_item(repo: &Path) -> Option<PunchItem> {
    absence_item(
        repo,
        TASK_DISCOVERY_SURFACES,
        "repository has no task-discovery surface (issue templates / in-repo tracker)",
        FindingKind::TaskDiscoverySurface,
        "task-discovery surfaces absent",
    )
}

/// The shared shape of the fixed-path marker-family probes: abstain when any
/// marker in the family exists at its path relative to `repo`, otherwise report
/// the one absent family as a neutral count of 1. Pure existence checks over a
/// documented set — no reads, no parsing, no error path.
fn absence_item(
    repo: &Path,
    markers: &[&str],
    title: &str,
    kind: FindingKind,
    unit: &str,
) -> Option<PunchItem> {
    if absence_count(repo, markers) == 0 {
        return None;
    }

    Some(PunchItem {
        title: title.to_string(),
        kind,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(1, unit),
        plane: None,
        subtree: None,
    })
}

/// The measured absence of a fixed-path marker family: `0` when any marker in the
/// family exists (the good thing is declared), `1` when none does. This is the
/// value [`absence_item`] emits (as a punch item only when it is `1`) and the
/// value [`super::structure_measurements`] reports (including the `0` the punch list
/// drops).
fn absence_count(repo: &Path, markers: &[&str]) -> u64 {
    u64::from(!markers.iter().any(|rel| repo.join(rel).exists()))
}

/// Whether the repo *declares* the generated-artifact-protection convention: a
/// `.gitattributes` at the repo root carrying at least one
/// [`LINGUIST_GENERATED_ATTR`] entry. The measure is the count of this single
/// well-known marker that is ABSENT (0 when declared, 1 when not) — the R6 "mark
/// generated files off-limits" signal that keeps an agent off derived state.
///
/// This reads one fixed-name config file and asks a purely structural question —
/// does the convention exist at all — and deliberately never decides *which*
/// files are generated (a semantic classification outside the audit's contract).
/// Born [`Tier::Tier3`]: a measured fact, not an evidence-backed best-practice.
/// A non-`.gitattributes` repo and a repo that has not adopted the convention are
/// the same positive fact (the marker is absent); only a declared marker abstains.
pub(super) fn generated_artifact_protection_item(
    repo: &Path,
) -> Result<Option<PunchItem>, AuditError> {
    if declares_linguist_generated(repo)? {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: "repository does not mark generated artifacts off-limits (.gitattributes \
                linguist-generated)"
            .to_string(),
        kind: FindingKind::GeneratedArtifactProtection,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(1, "protection markers absent"),
        plane: None,
        subtree: None,
    }))
}

/// Whether `repo`'s root `.gitattributes` declares any `linguist-generated`
/// attribute. Reads the one fixed-name file lossily (a stray non-UTF-8 byte
/// cannot abort the probe) and capped (a hostile multi-gigabyte attributes file
/// cannot exhaust memory); a missing file is `Ok(false)`, only a genuine IO error
/// (permissions, vanished file) propagates. The token match is a literal
/// substring — mechanical, not a parse of the attributes grammar.
fn declares_linguist_generated(repo: &Path) -> Result<bool, AuditError> {
    let path = repo.join(".gitattributes");
    if !path.exists() {
        return Ok(false);
    }
    match read_source_capped(&path)? {
        Some(text) => Ok(text.contains(LINGUIST_GENERATED_ATTR)),
        // Oversized attributes file: treat as undeclared rather than fail. The
        // measure is a structural proxy; an 8 MB `.gitattributes` is pathological.
        None => Ok(false),
    }
}

/// Count the well-known write-boundary declaration *kinds* that are ABSENT from
/// `repo` ([`WRITE_BOUNDARY_SURFACES`]). A repository declaring an ownership map
/// (CODEOWNERS) or a safe-write-zone policy is funnelling writes through a narrow,
/// auditable surface — the report's R5 "narrow mutation gateway / ownership
/// metadata" signal for "the safe place to write". The count is transparent
/// arithmetic over a fixed set (`k` absent of `N` known surfaces), the same
/// neutral-fact shape as the missing-enforcement-plane count; it asserts no
/// opinion beyond "these declared surfaces are absent". Abstains (emits nothing)
/// only when every known surface is present. Born [`Tier::Tier3`]; infallible —
/// every check is a fixed-path existence probe, so no IO error can arise.
pub(super) fn write_safety_zone_item(repo: &Path) -> Option<PunchItem> {
    let absent = absent_write_boundary_surfaces(repo);
    if absent.is_empty() {
        return None;
    }

    Some(PunchItem {
        title: format!(
            "write-boundary declaration surfaces absent: {}",
            absent.join(", ")
        ),
        kind: FindingKind::WriteSafetyZone,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(absent.len() as u64, "write-boundary surfaces absent"),
        plane: None,
        subtree: None,
    })
}

/// The measured count of write-boundary declaration families absent from `repo`
/// (0..=[`WRITE_BOUNDARY_SURFACES`]`.len()`). The value [`write_safety_zone_item`]
/// emits (as a punch item only when non-zero) and [`super::structure_measurements`]
/// reports (including `0`).
pub(super) fn write_boundary_absent_count(repo: &Path) -> u64 {
    absent_write_boundary_surfaces(repo).len() as u64
}

fn absent_write_boundary_surfaces(repo: &Path) -> Vec<&'static str> {
    WRITE_BOUNDARY_SURFACES
        .iter()
        .filter(|(_, candidates)| !candidates.iter().any(|rel| repo.join(rel).exists()))
        .map(|(surface, _)| *surface)
        .collect()
}

/// The measured count of dependency-pinning lockfile families absent from
/// `repo` — the corpus register of [`build_determinism_item`], which renders
/// the same count as a punch item.
pub(super) fn build_determinism_absent_count(repo: &Path) -> u64 {
    absence_count(repo, BUILD_DETERMINISM_MARKERS)
}

/// The measured count of reproducible-environment declarations absent from
/// `repo` — the corpus register of [`dev_environment_item`].
pub(super) fn dev_environment_absent_count(repo: &Path) -> u64 {
    absence_count(repo, DEV_ENVIRONMENT_MARKERS)
}

/// Whether `repo` fails to mark generated artifacts off-limits, as the 0/1 count
/// the corpus register wants — the counted form of
/// [`generated_artifact_protection_item`].
pub(super) fn generated_artifact_protection_absent_count(repo: &Path) -> Result<u64, AuditError> {
    Ok(u64::from(!declares_linguist_generated(repo)?))
}

/// The measured count of task-discovery surfaces absent from `repo` — the
/// corpus register of [`task_discovery_item`].
pub(super) fn task_discovery_absent_count(repo: &Path) -> u64 {
    absence_count(repo, TASK_DISCOVERY_SURFACES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::tests::tmp;
    use std::fs;

    #[test]
    fn build_determinism_item_when_no_lockfile_exists() {
        let dir = tmp("lock-none");
        fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();

        let item = build_determinism_item(&dir).expect("item");
        assert_eq!(item.kind, FindingKind::BuildDeterminism);
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "build-determinism markers absent");
        assert_eq!(item.measured_cost.value, 1);
        assert!(item.plane.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generated_protection_item_when_no_gitattributes() {
        let dir = tmp("gen-no-attrs");
        // A repo with no .gitattributes has not declared the convention: a fact.
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = generated_artifact_protection_item(&dir)
            .unwrap()
            .expect("item");
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.kind, FindingKind::GeneratedArtifactProtection);
        assert_eq!(item.measured_cost.unit, "protection markers absent");
        assert_eq!(item.measured_cost.value, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generated_protection_item_when_gitattributes_lacks_marker() {
        let dir = tmp("gen-attrs-no-marker");
        // A .gitattributes that declares other attributes but not linguist-generated
        // is still the convention absent.
        fs::write(dir.join(".gitattributes"), "*.rs text eol=lf\n").unwrap();

        let item = generated_artifact_protection_item(&dir)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_build_determinism_item_when_a_lockfile_exists() {
        // One representative marker per ecosystem family suffices: the probe is
        // an any-of over a fixed set.
        for marker in ["Cargo.lock", "package-lock.json", "go.sum", "flake.lock"] {
            let dir = tmp(&format!("lock-{}", marker.replace('.', "-")));
            fs::write(dir.join(marker), "").unwrap();
            assert!(
                build_determinism_item(&dir).is_none(),
                "{marker} pins the build -> no finding"
            );
            fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn build_determinism_probes_the_root_only() {
        // A lockfile buried in a subdirectory is a member's pin, not the repo's
        // front-door declaration; the probe is a fixed-path root existence check.
        let dir = tmp("lock-nested");
        let sub = dir.join("crates").join("foo");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("Cargo.lock"), "").unwrap();

        assert!(build_determinism_item(&dir).is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dev_environment_item_when_no_declaration_exists() {
        let dir = tmp("devenv-none");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = dev_environment_item(&dir).expect("item");
        assert_eq!(item.kind, FindingKind::DevEnvironmentDeclaration);
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(
            item.measured_cost.unit,
            "dev-environment declarations absent"
        );
        assert_eq!(item.measured_cost.value, 1);
        assert!(item.plane.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_dev_environment_item_when_a_declaration_exists() {
        for marker in [
            "flake.nix",
            "rust-toolchain.toml",
            ".tool-versions",
            ".nvmrc",
        ] {
            let dir = tmp(&format!("devenv-{}", marker.replace('.', "-")));
            fs::write(dir.join(marker), "").unwrap();
            assert!(
                dev_environment_item(&dir).is_none(),
                "{marker} declares the dev environment -> no finding"
            );
            fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn no_dev_environment_item_when_a_devcontainer_is_declared() {
        // The devcontainer marker is a fixed nested path, not a root filename.
        let dir = tmp("devenv-devcontainer");
        let dc = dir.join(".devcontainer");
        fs::create_dir_all(&dc).unwrap();
        fs::write(dc.join("devcontainer.json"), "{}\n").unwrap();

        assert!(dev_environment_item(&dir).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn task_discovery_item_when_no_surface_exists() {
        let dir = tmp("task-none");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = task_discovery_item(&dir).expect("item");
        assert_eq!(item.kind, FindingKind::TaskDiscoverySurface);
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.measured_cost.unit, "task-discovery surfaces absent");
        assert_eq!(item.measured_cost.value, 1);
        assert!(item.plane.is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_task_discovery_item_when_an_issue_template_dir_exists() {
        // The canonical GitHub form: a directory of templates under .github/.
        let dir = tmp("task-gh-dir");
        let templates = dir.join(".github").join("ISSUE_TEMPLATE");
        fs::create_dir_all(&templates).unwrap();
        fs::write(templates.join("bug.md"), "# bug\n").unwrap();

        assert!(task_discovery_item(&dir).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_generated_protection_item_when_marker_declared() {
        let dir = tmp("gen-marker-present");
        fs::write(
            dir.join(".gitattributes"),
            "src/generated/** linguist-generated=true\n",
        )
        .unwrap();

        assert!(generated_artifact_protection_item(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generated_protection_reads_non_utf8_gitattributes_without_aborting() {
        let dir = tmp("gen-non-utf8");
        // A .gitattributes with a stray non-UTF-8 byte must not abort the probe;
        // it simply does not contain the marker, so the convention is absent.
        fs::write(dir.join(".gitattributes"), [0xff, b'\n']).unwrap();

        let item = generated_artifact_protection_item(&dir)
            .unwrap()
            .expect("item");
        assert_eq!(item.measured_cost.value, 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_safety_item_when_no_boundary_declared() {
        let dir = tmp("write-none");
        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();

        let item = write_safety_zone_item(&dir).expect("item");
        assert_eq!(item.tier, Tier::Tier3);
        assert_eq!(item.kind, FindingKind::WriteSafetyZone);
        assert_eq!(item.measured_cost.unit, "write-boundary surfaces absent");
        assert_eq!(item.measured_cost.value, 2);
        assert_eq!(
            item.title,
            "write-boundary declaration surfaces absent: CODEOWNERS, \
             AOA safe-write-zone policy (.aoa/write-policy.toml)"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_task_discovery_item_when_a_single_file_template_exists() {
        let dir = tmp("task-gh-file");
        let gh = dir.join(".github");
        fs::create_dir_all(&gh).unwrap();
        fs::write(gh.join("ISSUE_TEMPLATE.md"), "# template\n").unwrap();

        assert!(task_discovery_item(&dir).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_task_discovery_item_when_a_beads_tracker_exists() {
        // The in-repo issue-tracker convention (same ecosystem precedent as the
        // .aoa dir elsewhere in the family): a .beads dir is a discoverable
        // task-discovery surface.
        let dir = tmp("task-beads");
        fs::create_dir_all(dir.join(".beads")).unwrap();

        assert!(task_discovery_item(&dir).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_safety_counts_one_absent_when_codeowners_present() {
        let dir = tmp("write-codeowners");
        // A CODEOWNERS at one of the documented locations satisfies the ownership
        // surface; only the safe-write-policy surface remains absent.
        let gh = dir.join(".github");
        fs::create_dir_all(&gh).unwrap();
        fs::write(gh.join("CODEOWNERS"), "* @owner\n").unwrap();

        let item = write_safety_zone_item(&dir).expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(
            item.title,
            "write-boundary declaration surfaces absent: \
             AOA safe-write-zone policy (.aoa/write-policy.toml)"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_safety_counts_one_absent_when_policy_file_present() {
        let dir = tmp("write-policy");
        // The mirror of the CODEOWNERS quadrant: a .aoa/write-policy.toml
        // satisfies the safe-write-policy surface, so only the ownership-map
        // surface remains absent.
        let aoa = dir.join(".aoa");
        fs::create_dir_all(&aoa).unwrap();
        fs::write(aoa.join("write-policy.toml"), "[zones]\n").unwrap();

        let item = write_safety_zone_item(&dir).expect("item");
        assert_eq!(item.measured_cost.value, 1);
        assert_eq!(
            item.title,
            "write-boundary declaration surfaces absent: CODEOWNERS"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_write_safety_item_when_all_surfaces_declared() {
        let dir = tmp("write-all");
        fs::write(dir.join("CODEOWNERS"), "* @owner\n").unwrap();
        let aoa = dir.join(".aoa");
        fs::create_dir_all(&aoa).unwrap();
        fs::write(aoa.join("write-policy.toml"), "[zones]\n").unwrap();

        assert!(write_safety_zone_item(&dir).is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
