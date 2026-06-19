use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use aoa_audit::{audit, exit_code, observe, write_trace, AuditConfig, AuditReport, Tier};
use aoa_metrics::{IndexQuality, SymbolGraph};
use aoa_trace::{Span, SpanSource, SpanType, Trace};

use serde_json::Map;
use tempfile::TempDir;

/// Build a small fixture repo in a temp dir with a couple of tracked-style
/// files. Nothing here is a real git repo; the hermetic assertion is purely
/// over the set of files present.
fn fixture_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("create temp repo");
    std::fs::write(dir.path().join("AGENTS.md"), "# Agents\nSee @rules.md\n")
        .expect("write AGENTS.md");
    std::fs::write(dir.path().join("rules.md"), "rule one\nrule two\n").expect("write rules.md");
    std::fs::write(dir.path().join("src.rs"), "fn main() {}\n").expect("write src.rs");
    dir
}

/// Recursively collect every file path under `root`, relative to `root`.
fn file_set(root: &Path) -> HashSet<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut HashSet<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.insert(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = HashSet::new();
    walk(root, root, &mut out);
    out
}

/// Assert that the post-run file set added only paths under `.aoa/` and never
/// removed a pre-existing (tracked-style) file.
fn assert_only_aoa_added(before: &HashSet<PathBuf>, after: &HashSet<PathBuf>) {
    for path in before {
        assert!(
            after.contains(path),
            "pre-existing file {} was removed",
            path.display()
        );
    }
    for path in after.difference(before) {
        assert!(
            path.starts_with(".aoa"),
            "non-ignored file created: {}",
            path.display()
        );
    }
}

fn valid_trace() -> Trace {
    let span = |span_type, seq| Span {
        span_type,
        source: SpanSource::Native,
        seq,
        attributes: Map::new(),
    };
    Trace {
        spans: vec![
            span(SpanType::RetrievalSearch, 0),
            span(SpanType::FileRead, 1),
            span(SpanType::WriteAttempt, 2),
        ],
    }
}

/// A symbol graph with a non-empty writable mutation surface so the audit emits
/// a measurable mutation-surface cost.
fn graph_with_surface() -> SymbolGraph {
    let mut writable = BTreeSet::new();
    writable.insert("a".to_string());
    writable.insert("b".to_string());
    SymbolGraph {
        nodes: vec!["root".into(), "a".into(), "b".into()],
        edges: vec![("root".into(), "a".into()), ("a".into(), "b".into())],
        writable,
        quality: IndexQuality::BestEffort,
    }
}

fn audit_config() -> AuditConfig {
    AuditConfig {
        context_root: Some(PathBuf::from("AGENTS.md")),
        ceiling: 0,
        graph: graph_with_surface(),
        trace: valid_trace(),
        ..AuditConfig::default()
    }
}

// Criterion 1: observe writes nothing to tracked files; only .aoa/ may appear.
#[test]
fn observe_writes_only_ignored_aoa_tree() {
    let repo = fixture_repo();
    let before = file_set(repo.path());

    let outcome = observe(repo.path()).expect("observe succeeds");
    assert!(outcome.traces_dir.starts_with(repo.path()));

    let after = file_set(repo.path());
    assert_only_aoa_added(&before, &after);
}

// Criterion 2: the observe-installed path produces a valid trace.
#[test]
fn observe_path_produces_valid_trace() {
    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");

    let (path, report) =
        write_trace(&outcome, "run-1.json", &valid_trace()).expect("write + validate trace");

    assert!(path.starts_with(&outcome.traces_dir));
    assert!(report.total() >= 1, "expected at least one validated span");
    // Re-validate via the public aoa-trace entrypoint to prove the file on disk
    // is independently valid.
    aoa_trace::validate_trace(&path).expect("trace file validates standalone");
}

// Criterion 3: audit writes nothing to tracked files.
#[test]
fn audit_does_not_mutate_repo() {
    let repo = fixture_repo();
    let before = file_set(repo.path());

    let _report = audit(repo.path(), &audit_config()).expect("audit succeeds");

    let after = file_set(repo.path());
    assert_eq!(before, after, "audit must not change any file in the repo");
}

// Criterion 4: audit emits both a human punch-list with measured cost and JSON.
#[test]
fn audit_emits_human_and_json_renderings() {
    let repo = fixture_repo();
    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");

    let human = report.render_human();
    assert!(human.contains("punch-list"));
    assert!(human.contains("cost:"), "human render lacks measured cost");

    let json = serde_json::to_string(&report).expect("serialize report");
    let parsed: AuditReport = serde_json::from_str(&json).expect("deserialize report");
    assert_eq!(parsed, report);
}

// Criterion 5: every punch-list item is tagged Tier-1/2/3.
#[test]
fn every_item_has_a_tier() {
    let repo = fixture_repo();
    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");

    assert!(!report.items.is_empty(), "expected at least one punch item");
    for item in &report.items {
        assert!(matches!(item.tier, Tier::Tier1 | Tier::Tier2 | Tier::Tier3));
    }
}

// Criterion 6: exit code semantics across all 4 combinations.
#[test]
fn exit_code_table() {
    let tier1_item = aoa_audit::PunchItem {
        title: "tier1 gap".into(),
        tier: Tier::Tier1,
        measured_cost: aoa_audit::MeasuredCost::new(1, "missing plane"),
        plane: Some(aoa_audit::EnforcementPlane::RuntimeHook),
    };
    let tier2_item = aoa_audit::PunchItem {
        title: "tier2 gap".into(),
        tier: Tier::Tier2,
        measured_cost: aoa_audit::MeasuredCost::new(1, "missing plane"),
        plane: Some(aoa_audit::EnforcementPlane::PreCommit),
    };

    let with_tier1 = AuditReport::new(vec![tier1_item.clone(), tier2_item.clone()]);
    let without_tier1 = AuditReport::new(vec![tier2_item]);

    // (fail_on_tier1, tier1-present) -> expected exit code
    let cases = [
        (false, false, 0),
        (false, true, 0),
        (true, false, 0),
        (true, true, 2),
    ];

    for (fail_on_tier1, tier1_present, expected) in cases {
        let report = if tier1_present {
            &with_tier1
        } else {
            &without_tier1
        };
        assert_eq!(
            exit_code(report, fail_on_tier1),
            expected,
            "fail_on_tier1={fail_on_tier1}, tier1_present={tier1_present}"
        );
    }
}

// Defensive: the default-config audit (no context root match, empty graph) still
// produces a well-formed, ranked report with tiered items.
#[test]
fn default_audit_on_bare_repo_is_well_formed() {
    let repo = tempfile::tempdir().expect("temp repo");
    let report = audit(repo.path(), &AuditConfig::default()).expect("audit succeeds");

    assert!(!report.items.is_empty());
    // Ranking: tiers are non-decreasing across the list.
    for pair in report.items.windows(2) {
        assert!(pair[0].tier <= pair[1].tier, "items not ranked by tier");
    }
}
