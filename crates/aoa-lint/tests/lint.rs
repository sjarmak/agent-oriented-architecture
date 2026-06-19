use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_lint::{lint_context, Finding, LintReport, SmellCategory};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tree/AGENTS.md")
}

fn run() -> LintReport {
    lint_context(&fixture_root(), "o200k_base").expect("lint_context should succeed")
}

/// Criterion 1: detect a set of smells over the fixture tree, with each finding
/// mapped to a 2606.15828 catalog category; assert >=3 DISTINCT smell types.
#[test]
fn detects_distinct_smell_categories_mapped_to_catalog() {
    let report = run();

    let categories: BTreeSet<&'static str> =
        report.findings.iter().map(|f| f.category.id()).collect();

    assert!(
        categories.len() >= 3,
        "expected >=3 distinct smell categories, got {categories:?}"
    );

    // Every finding carries a stable, non-empty catalog id (machine-readable
    // mapping to the taxonomy).
    for finding in &report.findings {
        assert!(
            !finding.category.id().is_empty(),
            "finding category id must be non-empty: {finding:?}"
        );
    }
}

/// Criterion 1 (specifics): the fixtures trigger each of the five catalog
/// categories at least once, confirming the mapping is concrete.
#[test]
fn fixture_triggers_each_catalog_category() {
    let report = run();
    let ids: BTreeSet<&'static str> = report.findings.iter().map(|f| f.category.id()).collect();

    for expected in [
        "contradiction",
        "duplication",
        "verbosity",
        "stale_reference",
        "overbroad_glob",
    ] {
        assert!(
            ids.contains(expected),
            "missing category '{expected}' in {ids:?}"
        );
    }

    // Sanity-check the enum ids are stable.
    assert_eq!(SmellCategory::Contradiction.id(), "contradiction");
    assert_eq!(SmellCategory::Duplication.id(), "duplication");
    assert_eq!(SmellCategory::Verbosity.id(), "verbosity");
    assert_eq!(SmellCategory::StaleReference.id(), "stale_reference");
    assert_eq!(SmellCategory::OverBroadGlob.id(), "overbroad_glob");
}

/// Criterion 2: the report composes the aoa-budget closure result (resolved
/// file set + token budget) with the smell findings in a SINGLE struct.
#[test]
fn report_composes_budget_and_findings() {
    let report = run();

    // Budget section present: the closure resolved at least the root + the
    // linked rules/README.md, and token totals are populated.
    assert!(
        report.budget.files.len() >= 2,
        "budget should include the resolved closure file set, got {}",
        report.budget.files.len()
    );
    assert!(
        report.budget.target_tokens > 0,
        "budget token total should be counted"
    );
    assert_eq!(report.budget.target_model, "o200k_base");

    // Findings section present.
    assert!(
        !report.findings.is_empty(),
        "findings section should be populated"
    );
}

/// Criterion 2 (reuse): the linted files are exactly the budget closure's files
/// — lint reuses the aoa-budget closure to decide WHAT to lint.
#[test]
fn linted_files_come_from_budget_closure() {
    let report = run();

    let budget_files: BTreeSet<&PathBuf> = report.budget.files.iter().map(|f| &f.path).collect();
    for finding in &report.findings {
        assert!(
            budget_files.contains(&finding.file),
            "finding file {:?} is not in the budget closure",
            finding.file
        );
    }
}

/// Criterion 3: each finding carries file path, human-readable message, AND a
/// machine-readable category.
#[test]
fn finding_has_path_message_and_category() {
    let report = run();
    let finding: &Finding = report.findings.first().expect("at least one finding");

    assert!(
        !finding.file.as_os_str().is_empty(),
        "finding must carry a file path"
    );
    assert!(
        !finding.message.trim().is_empty(),
        "finding must carry a message"
    );
    assert!(
        !finding.category.id().is_empty(),
        "finding must carry a category id"
    );
}

/// The report round-trips through serde_json (it is a single structured report).
#[test]
fn report_serializes_to_json() {
    let report = run();
    let json = serde_json::to_string(&report).expect("serialize");
    assert!(json.contains("\"budget\""));
    assert!(json.contains("\"findings\""));

    let back: LintReport = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.findings.len(), report.findings.len());
}

/// An unknown target tokenizer fails the lint loudly rather than guessing.
#[test]
fn unknown_tokenizer_errors() {
    let result = lint_context(&fixture_root(), "not-a-real-tokenizer");
    assert!(result.is_err(), "unknown tokenizer should error");
}
