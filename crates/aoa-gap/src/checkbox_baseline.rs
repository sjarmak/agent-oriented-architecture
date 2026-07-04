//! Factory agent-readiness level as a checkbox-baseline feature column (R0 study).
//!
//! AOA's primary claim is that file-presence checkboxes do not predict measured
//! agent success. Factory's published Agent Readiness Model (5 maturity levels,
//! 9 pillars, 80%-progression rule) turns that claim into a runnable experiment:
//! score each R0 experiment repo on Factory's criteria and carry the resulting
//! level as a baseline feature column next to AOA's trace metrics in the
//! correlation machinery. A null result for this column IS the headline.
//!
//! # Criteria provenance (PROVISIONAL)
//!
//! Factory publishes the model shape but not an exhaustive per-level criteria
//! list (see [`FACTORY_CRITERIA_SOURCE`]). The table here is therefore
//! **provisional**: it transcribes every criterion Factory names as an example
//! (lint config, type checker, pre-commit hooks, CODEOWNERS, AGENTS.md, README,
//! devcontainers, issue/PR templates, branch protection, secret scanning) and
//! fills each pillar with conventional fixed-path signals at the level Factory's
//! own level descriptions place them. The version is pinned in
//! [`FACTORY_CRITERIA_VERSION`]; a follow-up bead tracks replacing this table
//! with Factory's official criteria once published.
//!
//! # Mechanism, not judgment (ZFC)
//!
//! Every evaluated criterion is a fixed relative-path existence check against
//! the repo tree — the same discipline as `aoa-audit`'s structure probes.
//! Criteria that would need semantic or organizational judgment (documentation
//! freshness, structured-logging quality, branch protection, secret scanning,
//! analytics infrastructure, self-improving orchestration) are carried with an
//! explicit `Excluded` status and reason — never silently dropped — and are
//! removed from every pass-fraction denominator.
//!
//! # Deliberately NOT a gating candidate
//!
//! The checkbox level is a *study baseline*, not a candidate metric: it is kept
//! out of `GATING_CANDIDATES` (src/construct.rs) on purpose. Promoting it there
//! would let a checkbox score gate decisions, which is exactly the practice the
//! study interrogates.
//!
//! # Scope
//!
//! Scoring is repo-scope only (paths relative to the supplied root). Factory's
//! application-scope per-app evaluation for monorepos ("3/4" scores) is out of
//! scope here. The corpus join against hidden-set outcomes is also NOT built in
//! this module: the external-outcome corpus layer lives on an unmerged branch,
//! and the join is a follow-up bead blocked on that merge. [`CheckboxBaseline`]
//! is shaped (repo id, level, per-pillar fractions, pinned criteria version) so
//! that join can carry it as-is.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Pinned version of the criteria table below. `provisional`: derived from
/// Factory's published model shape and example criteria, not an official
/// per-level criteria list (Factory has not published one).
pub const FACTORY_CRITERIA_VERSION: &str = "factory-provisional-2026-07-04";

/// Where the criteria table came from and when it was retrieved.
pub const FACTORY_CRITERIA_SOURCE: &str =
    "https://docs.factory.ai/web/agent-readiness/overview.md, \
https://docs.factory.ai/cli/features/readiness-report.md, \
https://docs.factory.ai/reference/readiness-reports-api.md (retrieved 2026-07-04). \
Factory publishes the 5 levels, the 80%-of-previous-level progression rule, the 9 pillars, \
and example criteria only; no exhaustive per-level criteria list is published, so this \
table is provisional.";

/// Factory's five maturity level names, index 0 = level 1.
pub const FACTORY_LEVEL_NAMES: [&str; 5] = [
    "Functional",
    "Documented",
    "Standardized",
    "Optimized",
    "Autonomous",
];

/// Factory's nine technical pillars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pillar {
    StyleAndValidation,
    BuildSystem,
    Testing,
    Documentation,
    DevelopmentEnvironment,
    DebuggingAndObservability,
    Security,
    TaskDiscovery,
    ProductAndExperimentation,
}

/// All nine pillars in Factory's published order.
pub const PILLARS: [Pillar; 9] = [
    Pillar::StyleAndValidation,
    Pillar::BuildSystem,
    Pillar::Testing,
    Pillar::Documentation,
    Pillar::DevelopmentEnvironment,
    Pillar::DebuggingAndObservability,
    Pillar::Security,
    Pillar::TaskDiscovery,
    Pillar::ProductAndExperimentation,
];

/// How a criterion is evaluated: a mechanical fixed-path existence check, or an
/// explicit exclusion (with reason) for criteria that cannot be decided from
/// the repo tree alone.
enum Check {
    /// Passes when ANY of the listed paths (relative to the repo root) exists.
    AnyPathExists(&'static [&'static str]),
    /// Not mechanically checkable from the repo tree; carried, never scored.
    Excluded(&'static str),
}

/// One provisional Factory readiness criterion.
struct Criterion {
    id: &'static str,
    pillar: Pillar,
    /// Maturity level (1..=5) this criterion belongs to.
    level: u8,
    check: Check,
}

/// The provisional criteria table. See the module docs for provenance and the
/// exclusion policy; see [`FACTORY_CRITERIA_VERSION`] for the pinned version.
const CRITERIA: &[Criterion] = &[
    // ---- Level 1: Functional — code runs, basic tooling in place ----
    Criterion {
        id: "build_manifest",
        pillar: Pillar::BuildSystem,
        level: 1,
        check: Check::AnyPathExists(&[
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "setup.py",
            "go.mod",
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "Gemfile",
            "Makefile",
            "CMakeLists.txt",
        ]),
    },
    Criterion {
        id: "readme",
        pillar: Pillar::Documentation,
        level: 1,
        check: Check::AnyPathExists(&["README.md", "README", "README.rst", "README.txt"]),
    },
    Criterion {
        id: "lint_config",
        pillar: Pillar::StyleAndValidation,
        level: 1,
        check: Check::AnyPathExists(&[
            ".eslintrc.json",
            ".eslintrc.js",
            ".eslintrc.yml",
            "eslint.config.js",
            "eslint.config.mjs",
            "ruff.toml",
            ".ruff.toml",
            ".flake8",
            ".golangci.yml",
            ".golangci.yaml",
            "clippy.toml",
            ".clippy.toml",
            ".rubocop.yml",
        ]),
    },
    Criterion {
        id: "test_directory",
        pillar: Pillar::Testing,
        level: 1,
        check: Check::AnyPathExists(&["tests", "test", "spec", "__tests__"]),
    },
    // ---- Level 2: Documented — process and documentation established ----
    Criterion {
        id: "agent_instructions",
        pillar: Pillar::Documentation,
        level: 2,
        check: Check::AnyPathExists(&["AGENTS.md", "CLAUDE.md", ".cursorrules"]),
    },
    Criterion {
        id: "contributing_guide",
        pillar: Pillar::Documentation,
        level: 2,
        check: Check::AnyPathExists(&[
            "CONTRIBUTING.md",
            "docs/CONTRIBUTING.md",
            ".github/CONTRIBUTING.md",
        ]),
    },
    Criterion {
        id: "formatter_config",
        pillar: Pillar::StyleAndValidation,
        level: 2,
        check: Check::AnyPathExists(&[
            ".prettierrc",
            ".prettierrc.json",
            ".prettierrc.yml",
            "prettier.config.js",
            "rustfmt.toml",
            ".rustfmt.toml",
            ".editorconfig",
            ".clang-format",
        ]),
    },
    Criterion {
        id: "ci_workflow",
        pillar: Pillar::BuildSystem,
        level: 2,
        check: Check::AnyPathExists(&[
            ".github/workflows",
            ".gitlab-ci.yml",
            ".circleci/config.yml",
            "Jenkinsfile",
            ".buildkite",
        ]),
    },
    Criterion {
        id: "documentation_freshness",
        pillar: Pillar::Documentation,
        level: 2,
        check: Check::Excluded(
            "documentation freshness is a semantic judgment over content and history, \
             not a fixed-path fact",
        ),
    },
    // ---- Level 3: Standardized — enforced automation, security configured ----
    Criterion {
        id: "type_checker_config",
        pillar: Pillar::StyleAndValidation,
        level: 3,
        check: Check::AnyPathExists(&[
            "tsconfig.json",
            "mypy.ini",
            ".mypy.ini",
            "pyrightconfig.json",
        ]),
    },
    Criterion {
        id: "precommit_hooks",
        pillar: Pillar::StyleAndValidation,
        level: 3,
        check: Check::AnyPathExists(&[
            ".pre-commit-config.yaml",
            ".husky",
            "lefthook.yml",
            ".lefthook.yml",
        ]),
    },
    Criterion {
        id: "codeowners",
        pillar: Pillar::Security,
        level: 3,
        check: Check::AnyPathExists(&["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"]),
    },
    Criterion {
        id: "issue_templates",
        pillar: Pillar::TaskDiscovery,
        level: 3,
        check: Check::AnyPathExists(&[
            ".github/ISSUE_TEMPLATE",
            ".github/ISSUE_TEMPLATE.md",
            ".gitlab/issue_templates",
        ]),
    },
    Criterion {
        id: "pr_template",
        pillar: Pillar::TaskDiscovery,
        level: 3,
        check: Check::AnyPathExists(&[
            ".github/PULL_REQUEST_TEMPLATE.md",
            ".github/pull_request_template.md",
            "PULL_REQUEST_TEMPLATE.md",
            "docs/PULL_REQUEST_TEMPLATE.md",
        ]),
    },
    Criterion {
        id: "reproducible_environment",
        pillar: Pillar::DevelopmentEnvironment,
        level: 3,
        check: Check::AnyPathExists(&[
            ".devcontainer",
            ".devcontainer.json",
            "Dockerfile",
            "docker-compose.yml",
            "compose.yaml",
            "flake.nix",
            "shell.nix",
            ".tool-versions",
        ]),
    },
    Criterion {
        id: "security_policy",
        pillar: Pillar::Security,
        level: 3,
        check: Check::AnyPathExists(&["SECURITY.md", ".github/SECURITY.md", "docs/SECURITY.md"]),
    },
    Criterion {
        id: "branch_protection",
        pillar: Pillar::Security,
        level: 3,
        check: Check::Excluded(
            "branch protection is a hosting-platform setting, not visible in the repo tree",
        ),
    },
    Criterion {
        id: "secret_scanning",
        pillar: Pillar::Security,
        level: 3,
        check: Check::Excluded(
            "secret scanning is an org/platform setting, not visible in the repo tree",
        ),
    },
    // ---- Level 4: Optimized — fast feedback and continuous measurement ----
    Criterion {
        id: "coverage_config",
        pillar: Pillar::Testing,
        level: 4,
        check: Check::AnyPathExists(&[
            "codecov.yml",
            ".codecov.yml",
            ".coveragerc",
            "tarpaulin.toml",
            ".tarpaulin.toml",
        ]),
    },
    Criterion {
        id: "benchmarks",
        pillar: Pillar::Testing,
        level: 4,
        check: Check::AnyPathExists(&["benches", "benchmarks", "benchmark"]),
    },
    Criterion {
        id: "dependency_automation",
        pillar: Pillar::BuildSystem,
        level: 4,
        check: Check::AnyPathExists(&[
            ".github/dependabot.yml",
            ".github/dependabot.yaml",
            "renovate.json",
            ".renovaterc.json",
        ]),
    },
    Criterion {
        id: "structured_observability",
        pillar: Pillar::DebuggingAndObservability,
        level: 4,
        check: Check::Excluded(
            "structured logging/tracing/metrics quality is a runtime and code-semantics \
             judgment, not a fixed-path fact",
        ),
    },
    // ---- Level 5: Autonomous — self-improving systems ----
    Criterion {
        id: "analytics_infrastructure",
        pillar: Pillar::ProductAndExperimentation,
        level: 5,
        check: Check::Excluded(
            "analytics/experimentation infrastructure is organizational, not visible in \
             the repo tree",
        ),
    },
    Criterion {
        id: "self_improving_orchestration",
        pillar: Pillar::ProductAndExperimentation,
        level: 5,
        check: Check::Excluded(
            "self-improving orchestration is an organizational capability, not a \
             fixed-path fact",
        ),
    },
];

/// Why a repo could not be scored.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CheckboxBaselineError {
    /// The supplied repo root does not exist or is not a directory.
    #[error("repo root `{0}` is not a directory")]
    RootNotADirectory(String),
}

/// Outcome of one criterion against one repo tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CriterionStatus {
    /// At least one of the criterion's fixed paths exists.
    Pass,
    /// None of the criterion's fixed paths exist.
    Fail,
    /// Not mechanically checkable from the repo tree; never enters a denominator.
    Excluded { reason: String },
}

/// One criterion's identity and outcome, carried in full so an excluded
/// criterion is visible in the artifact rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionResult {
    pub id: String,
    pub pillar: Pillar,
    pub level: u8,
    pub status: CriterionStatus,
}

/// Pass counts for one maturity level. `evaluated` counts mechanical criteria
/// only; excluded criteria are tallied separately and never enter the fraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelScore {
    pub level: u8,
    pub name: String,
    pub passed: usize,
    pub evaluated: usize,
    pub excluded: usize,
    /// `passed / evaluated`; `None` when the level has no mechanical criteria.
    pub pass_fraction: Option<f64>,
}

/// Pass counts for one pillar, same denominator policy as [`LevelScore`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PillarScore {
    pub pillar: Pillar,
    pub passed: usize,
    pub evaluated: usize,
    pub excluded: usize,
    pub pass_fraction: Option<f64>,
}

/// The checkbox-baseline feature column for one repo: the Factory level under
/// the 80%-progression rule plus the per-level and per-pillar fractions behind
/// it, stamped with the pinned criteria version so a future corpus join can
/// carry it alongside hidden-set outcomes reproducibly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckboxBaseline {
    pub repo_id: String,
    pub criteria_version: String,
    pub criteria_source: String,
    /// Achieved maturity level in 1..=5.
    pub level: u8,
    pub levels: Vec<LevelScore>,
    pub pillars: Vec<PillarScore>,
    pub criteria: Vec<CriterionResult>,
}

/// Factory's progression gate: pass at least 80% of a level's criteria.
/// Integer arithmetic (`passed / evaluated >= 4/5`) so an exact-80% boundary
/// cannot be lost to float rounding. A level with no mechanical criteria cannot
/// be verified as passed.
fn passes_progression(passed: usize, evaluated: usize) -> bool {
    evaluated > 0 && passed * 5 >= evaluated * 4
}

/// Tally `(passed, evaluated, excluded)` over a subset of criterion results;
/// excluded criteria never enter the `evaluated` denominator.
fn tally<'a>(results: impl Iterator<Item = &'a CriterionResult>) -> (usize, usize, usize) {
    let mut passed = 0;
    let mut evaluated = 0;
    let mut excluded = 0;
    for r in results {
        match r.status {
            CriterionStatus::Pass => {
                passed += 1;
                evaluated += 1;
            }
            CriterionStatus::Fail => evaluated += 1,
            CriterionStatus::Excluded { .. } => excluded += 1,
        }
    }
    (passed, evaluated, excluded)
}

/// `passed / evaluated` for reporting; `None` when nothing was evaluated.
fn fraction(passed: usize, evaluated: usize) -> Option<f64> {
    (evaluated > 0).then(|| passed as f64 / evaluated as f64)
}

/// The achieved level under Factory's gated progression: every repo starts at
/// level 1, and each consecutive level 1..=4 whose criteria clear the 80% gate
/// unlocks the next; the first failed rung halts progression (level-4 evidence
/// cannot skip a broken level-1 gate). Level-5 criteria gate nothing — there is
/// no level 6.
fn achieved_level(levels: &[LevelScore]) -> u8 {
    let mut achieved = 1u8;
    for score in levels.iter().filter(|l| l.level <= 4) {
        if passes_progression(score.passed, score.evaluated) {
            achieved = score.level + 1;
        } else {
            break;
        }
    }
    achieved
}

/// Score a repo tree against the provisional Factory criteria table. Every
/// evaluated criterion is a fixed relative-path existence check against `root`
/// (repo scope only); excluded criteria are carried with their reasons.
pub fn score_repo(
    repo_id: impl Into<String>,
    root: &Path,
) -> Result<CheckboxBaseline, CheckboxBaselineError> {
    if !root.is_dir() {
        return Err(CheckboxBaselineError::RootNotADirectory(
            root.display().to_string(),
        ));
    }

    let criteria: Vec<CriterionResult> = CRITERIA
        .iter()
        .map(|c| {
            let status = match c.check {
                Check::AnyPathExists(paths) => {
                    if paths.iter().any(|p| root.join(p).exists()) {
                        CriterionStatus::Pass
                    } else {
                        CriterionStatus::Fail
                    }
                }
                Check::Excluded(reason) => CriterionStatus::Excluded {
                    reason: reason.to_string(),
                },
            };
            CriterionResult {
                id: c.id.to_string(),
                pillar: c.pillar,
                level: c.level,
                status,
            }
        })
        .collect();

    let levels: Vec<LevelScore> = (1..=5u8)
        .map(|level| {
            let (passed, evaluated, excluded) = tally(criteria.iter().filter(|c| c.level == level));
            LevelScore {
                level,
                name: FACTORY_LEVEL_NAMES[usize::from(level) - 1].to_string(),
                passed,
                evaluated,
                excluded,
                pass_fraction: fraction(passed, evaluated),
            }
        })
        .collect();

    let pillars: Vec<PillarScore> = PILLARS
        .iter()
        .map(|&pillar| {
            let (passed, evaluated, excluded) =
                tally(criteria.iter().filter(|c| c.pillar == pillar));
            PillarScore {
                pillar,
                passed,
                evaluated,
                excluded,
                pass_fraction: fraction(passed, evaluated),
            }
        })
        .collect();

    let level = achieved_level(&levels);
    Ok(CheckboxBaseline {
        repo_id: repo_id.into(),
        criteria_version: FACTORY_CRITERIA_VERSION.to_string(),
        criteria_source: FACTORY_CRITERIA_SOURCE.to_string(),
        level,
        levels,
        pillars,
        criteria,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    /// Create every listed file (and its parent dirs) under `root`. A trailing
    /// `/` marks a directory instead of a file.
    fn touch_all(root: &Path, paths: &[&str]) {
        for p in paths {
            if let Some(dir) = p.strip_suffix('/') {
                fs::create_dir_all(root.join(dir)).unwrap();
            } else {
                let full = root.join(p);
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(full, "x").unwrap();
            }
        }
    }

    fn status_of<'a>(baseline: &'a CheckboxBaseline, id: &str) -> &'a CriterionStatus {
        &baseline
            .criteria
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("criterion {id} missing from output"))
            .status
    }

    fn level_score(baseline: &CheckboxBaseline, level: u8) -> &LevelScore {
        baseline
            .levels
            .iter()
            .find(|l| l.level == level)
            .unwrap_or_else(|| panic!("level {level} missing from output"))
    }

    /// Files satisfying all four level-1 mechanical criteria.
    const LEVEL1_FILES: &[&str] = &["Cargo.toml", "README.md", "clippy.toml", "tests/"];
    /// Files satisfying all four level-2 mechanical criteria.
    const LEVEL2_FILES: &[&str] = &[
        "AGENTS.md",
        "CONTRIBUTING.md",
        "rustfmt.toml",
        ".github/workflows/",
    ];
    /// Files satisfying six of the seven level-3 mechanical criteria
    /// (6/7 >= 80%; `type_checker_config` deliberately absent).
    const LEVEL3_SIX_OF_SEVEN: &[&str] = &[
        ".pre-commit-config.yaml",
        ".github/CODEOWNERS",
        ".github/ISSUE_TEMPLATE/",
        ".github/PULL_REQUEST_TEMPLATE.md",
        ".devcontainer/",
        "SECURITY.md",
    ];

    #[test]
    fn bare_repo_scores_level_one() {
        let dir = tempfile::tempdir().unwrap();
        let b = score_repo("bare", dir.path()).unwrap();
        assert_eq!(b.level, 1, "empty tree must sit at the level-1 floor");
        assert!(b
            .criteria
            .iter()
            .filter(|c| !matches!(c.status, CriterionStatus::Excluded { .. }))
            .all(|c| c.status == CriterionStatus::Fail));
        assert_eq!(b.repo_id, "bare");
        assert_eq!(b.criteria_version, FACTORY_CRITERIA_VERSION);
        assert_eq!(b.criteria_source, FACTORY_CRITERIA_SOURCE);
    }

    #[test]
    fn passing_level_one_criteria_unlocks_level_two() {
        let dir = tempfile::tempdir().unwrap();
        touch_all(dir.path(), LEVEL1_FILES);
        let b = score_repo("l2", dir.path()).unwrap();
        assert_eq!(b.level, 2);
        let l1 = level_score(&b, 1);
        assert_eq!((l1.passed, l1.evaluated), (4, 4));
        assert_eq!(l1.pass_fraction, Some(1.0));
    }

    #[test]
    fn level_three_repo_passes_one_and_two_but_not_three() {
        let dir = tempfile::tempdir().unwrap();
        touch_all(dir.path(), LEVEL1_FILES);
        touch_all(dir.path(), LEVEL2_FILES);
        // Only 2 of 7 level-3 mechanical criteria: below the 80% gate.
        touch_all(dir.path(), &[".github/CODEOWNERS", "SECURITY.md"]);
        let b = score_repo("l3", dir.path()).unwrap();
        assert_eq!(b.level, 3);
        let l3 = level_score(&b, 3);
        assert_eq!((l3.passed, l3.evaluated, l3.excluded), (2, 7, 2));
    }

    #[test]
    fn six_of_seven_meets_the_eighty_percent_gate() {
        // 6/7 ~ 0.857 >= 0.8 unlocks level 4; level-4 criteria all absent, so
        // the repo sits at exactly level 4.
        let dir = tempfile::tempdir().unwrap();
        touch_all(dir.path(), LEVEL1_FILES);
        touch_all(dir.path(), LEVEL2_FILES);
        touch_all(dir.path(), LEVEL3_SIX_OF_SEVEN);
        let b = score_repo("l4", dir.path()).unwrap();
        assert_eq!(b.level, 4);
    }

    #[test]
    fn progression_gate_is_exact_integer_arithmetic() {
        // Exact 80% passes; just below does not. Integer form avoids float
        // rounding on the 4/5 boundary.
        assert!(passes_progression(4, 5));
        assert!(passes_progression(80, 100));
        assert!(!passes_progression(79, 100));
        assert!(!passes_progression(3, 4)); // 75%
        assert!(passes_progression(6, 7)); // ~85.7%
        assert!(!passes_progression(5, 7)); // ~71.4%
        assert!(!passes_progression(0, 0), "empty level is unverifiable");
    }

    #[test]
    fn full_tree_reaches_level_five_and_skipping_a_level_halts_progression() {
        let dir = tempfile::tempdir().unwrap();
        touch_all(dir.path(), LEVEL1_FILES);
        touch_all(dir.path(), LEVEL2_FILES);
        touch_all(dir.path(), LEVEL3_SIX_OF_SEVEN);
        touch_all(
            dir.path(),
            &["codecov.yml", "benches/", ".github/dependabot.yml"],
        );
        let b = score_repo("l5", dir.path()).unwrap();
        assert_eq!(b.level, 5, "passing levels 1-4 reaches Autonomous");

        // Same tree minus ALL level-1 files: level 4 evidence cannot skip the
        // broken level-1 gate.
        let dir2 = tempfile::tempdir().unwrap();
        touch_all(dir2.path(), LEVEL2_FILES);
        touch_all(dir2.path(), LEVEL3_SIX_OF_SEVEN);
        touch_all(
            dir2.path(),
            &["codecov.yml", "benches/", ".github/dependabot.yml"],
        );
        let b2 = score_repo("gap", dir2.path()).unwrap();
        assert_eq!(b2.level, 1, "gated progression: level 1 failed, so floor");
    }

    #[test]
    fn excluded_criteria_carry_reasons_and_never_enter_denominators() {
        let dir = tempfile::tempdir().unwrap();
        let b = score_repo("excl", dir.path()).unwrap();
        for id in [
            "documentation_freshness",
            "branch_protection",
            "secret_scanning",
            "structured_observability",
            "analytics_infrastructure",
            "self_improving_orchestration",
        ] {
            match status_of(&b, id) {
                CriterionStatus::Excluded { reason } => {
                    assert!(!reason.is_empty(), "{id} must carry a reason")
                }
                other => panic!("{id} must be excluded, got {other:?}"),
            }
        }
        // Level 5 has only excluded criteria: no denominator, no fraction.
        let l5 = level_score(&b, 5);
        assert_eq!((l5.passed, l5.evaluated, l5.excluded), (0, 0, 2));
        assert_eq!(l5.pass_fraction, None);
        // Security pillar: 4 criteria, 2 excluded -> denominator 2.
        let sec = b
            .pillars
            .iter()
            .find(|p| p.pillar == Pillar::Security)
            .unwrap();
        assert_eq!((sec.evaluated, sec.excluded), (2, 2));
    }

    #[test]
    fn monorepo_member_files_do_not_satisfy_repo_scope_criteria() {
        // Repo-scope semantics: criteria are checked at the supplied root only.
        // A lint config buried in a workspace member does not count.
        let dir = tempfile::tempdir().unwrap();
        touch_all(dir.path(), &["Cargo.toml", "README.md", "tests/"]);
        touch_all(
            dir.path(),
            &["crates/app/clippy.toml", "crates/app/Cargo.toml"],
        );
        let b = score_repo("mono", dir.path()).unwrap();
        assert_eq!(status_of(&b, "lint_config"), &CriterionStatus::Fail);
        assert_eq!(status_of(&b, "build_manifest"), &CriterionStatus::Pass);
        assert_eq!(b.level, 1, "3/4 on level 1 is below the 80% gate");
    }

    #[test]
    fn missing_root_is_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let err = score_repo("x", &missing).unwrap_err();
        assert_eq!(
            err,
            CheckboxBaselineError::RootNotADirectory(missing.display().to_string())
        );
    }

    #[test]
    fn baseline_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        touch_all(dir.path(), LEVEL1_FILES);
        let b = score_repo("json", dir.path()).unwrap();
        let json = serde_json::to_string(&b).unwrap();
        let back: CheckboxBaseline = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
        assert!(json.contains("factory-provisional-2026-07-04"));
    }

    #[test]
    fn criteria_table_invariants_hold() {
        // Unique ids.
        let mut ids: Vec<&str> = CRITERIA.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "criterion ids must be unique");
        // Levels in range; levels 1-4 each have mechanical criteria so the
        // progression gate is decidable at every rung.
        for c in CRITERIA {
            assert!((1..=5).contains(&c.level), "{} level out of range", c.id);
        }
        for level in 1..=4u8 {
            let mechanical = CRITERIA
                .iter()
                .filter(|c| c.level == level)
                .filter(|c| matches!(c.check, Check::AnyPathExists(_)))
                .count();
            assert!(mechanical > 0, "level {level} needs mechanical criteria");
        }
        // Every pillar is represented.
        for pillar in PILLARS {
            assert!(
                CRITERIA.iter().any(|c| c.pillar == pillar),
                "pillar {pillar:?} unrepresented"
            );
        }
    }
}
