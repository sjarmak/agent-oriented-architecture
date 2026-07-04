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
        kind: aoa_audit::FindingKind::MissingPlane,
        tier: Tier::Tier1,
        measured_cost: aoa_audit::MeasuredCost::new(1, "missing plane"),
        plane: Some(aoa_audit::EnforcementPlane::RuntimeHook),
    };
    let tier2_item = aoa_audit::PunchItem {
        title: "tier2 gap".into(),
        kind: aoa_audit::FindingKind::MissingPlane,
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

// Criterion 7: the code-structure audit family surfaces a measured, Tier-3
// (advisory) item. The fixture repo has no README at its root, so the
// navigability-anchor check must emit an item — alongside the planes/budget
// items — proving structure checks are wired into `audit()`.
#[test]
fn audit_surfaces_structure_family_items() {
    let repo = fixture_repo();
    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");

    let anchor = report
        .items
        .iter()
        .find(|item| item.measured_cost.unit == "package roots")
        .expect("expected a navigability-anchor structure item");

    // A measured fact (a real count), born advisory: structure best-practices
    // are never evidence-backed until R9c external-outcome correlation.
    assert!(
        anchor.measured_cost.value >= 1,
        "anchor count must be a real measured count"
    );
    assert_eq!(
        anchor.tier,
        Tier::Tier3,
        "structure items are born advisory (Tier-3)"
    );
    assert!(
        anchor.plane.is_none(),
        "a structure item is not plane-shaped"
    );
}

// aoa-d6t.23 criterion: a history-poor repo (no held-out behavioral signal)
// reports InsufficientData for the behavioral metrics with the reason — never
// a fabricated mutation-surface score and never a silent checkbox degradation.
#[test]
fn greenfield_repo_reports_insufficient_data_not_a_fabricated_score() {
    let repo = fixture_repo(); // no .aoa/traces -> zero observations
    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");

    assert_eq!(report.behavioral_signal.observations, 0);
    assert!(!report.behavioral_signal.is_sufficient());

    let note = report
        .insufficient_data
        .as_ref()
        .expect("insufficient-data note present");
    assert_eq!(note.reason, aoa_gap::INSUFFICIENT_DATA_REASON);
    assert_eq!(note.metrics, aoa_gap::BEHAVIORAL_METRICS);

    // No fabricated behavioral score: the mutation-surface item is absent.
    assert!(
        !report
            .items
            .iter()
            .any(|i| i.kind == aoa_audit::FindingKind::MutationSurface),
        "a repo with no behavioral signal must not carry a fabricated surface"
    );

    let human = report.render_human();
    assert!(human.contains("InsufficientData"), "{human}");
    assert!(human.contains(aoa_gap::INSUFFICIENT_DATA_REASON), "{human}");
}

// aoa-d6t.23 criterion: once enough observe-captured sessions accumulate under
// .aoa/traces, the behavioral metrics light up (the mutation-surface item is
// emitted again and the insufficient-data note disappears).
#[test]
fn accumulated_trace_corpus_lights_the_behavioral_metrics_up() {
    let repo = fixture_repo();
    let traces = repo.path().join(".aoa").join("traces");
    std::fs::create_dir_all(&traces).expect("create traces dir");
    let span = r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#;
    for i in 0..10 {
        std::fs::write(traces.join(format!("live-s{i}.jsonl")), format!("{span}\n"))
            .expect("write live log");
    }

    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");
    assert_eq!(report.behavioral_signal.observations, 10);
    assert!(report.behavioral_signal.is_sufficient());
    assert!(report.insufficient_data.is_none());
    assert!(
        report
            .items
            .iter()
            .any(|i| i.kind == aoa_audit::FindingKind::MutationSurface),
        "with sufficient signal the behavioral item is measured again"
    );
    assert!(!report.render_human().contains("InsufficientData"));
}

// A corrupt corpus file must fail the audit loudly, never under-count signal.
#[test]
fn corrupt_trace_corpus_fails_the_audit_loud() {
    let repo = fixture_repo();
    let traces = repo.path().join(".aoa").join("traces");
    std::fs::create_dir_all(&traces).expect("create traces dir");
    std::fs::write(traces.join("live-bad.jsonl"), "not json\n").expect("write corrupt log");

    let err = audit(repo.path(), &audit_config()).expect_err("corruption is loud");
    assert!(
        err.to_string().contains("live-bad.jsonl"),
        "error names the file: {err}"
    );
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
