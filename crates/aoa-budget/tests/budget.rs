use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_budget::{
    count_budget, fix_oversized, resolve_closure, BudgetError, Config, Verdict, REFERENCE_ENCODING,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Copy a fixture directory into a unique scratch dir under the crate target so
/// mutating tests (suppression, fix) never touch committed fixtures.
fn scratch(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!(
            "{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

// Criterion 1: transitive multi-hop closure A -> B -> C.
#[test]
fn resolves_multi_hop_closure() {
    let root = fixtures().join("closure/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();
    let names: BTreeSet<String> = closure
        .files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(names.contains("AGENTS.md"), "root (A) present");
    assert!(names.contains("README.md"), "hop B present");
    assert!(names.contains("deep.md"), "hop C present (multi-hop)");
    assert_eq!(closure.files.len(), 3, "exactly A, B, C reachable");
}

// Criterion 1 (negative): external links and anchors are not followed.
#[test]
fn skips_external_and_anchor_references() {
    let root = fixtures().join("closure/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();
    for f in &closure.files {
        let n = f.path.file_name().unwrap().to_string_lossy();
        assert!(n.ends_with(".md"), "only local md files: {n}");
    }
}

// Criterion 2: real tokenizer, BOTH o200k reference and target counts present.
#[test]
fn reports_dual_tokenizer_counts() {
    let root = fixtures().join("closure/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();
    let report = count_budget(&closure, "gpt-4o", &Config::blocking(100_000)).unwrap();

    assert!(report.o200k_tokens > 0, "o200k reference count present");
    assert!(report.target_tokens > 0, "target count present");
    assert_eq!(report.reference_encoding, REFERENCE_ENCODING);
    assert_eq!(report.target_model, "gpt-4o");
    // Per-file breakdown carries both counts too.
    for f in &report.files {
        assert!(f.o200k_tokens > 0 && f.target_tokens > 0);
    }
}

// Criterion 2 (cross-encoding): the reference total is target-independent, the
// o200k target mirrors it, and a genuinely different encoding (cl100k) is used
// for the cl100k target — proven on text where the two encodings diverge.
#[test]
fn target_tokenizer_uses_distinct_encoding_for_cl100k() {
    use aoa_budget::{count_tokens, target_encoder};

    // Multilingual text tokenizes far more efficiently under o200k than cl100k.
    let probe = "你好世界，这是中文测试文本";
    let o = target_encoder("gpt-4o").unwrap();
    let c = target_encoder("gpt-4").unwrap();
    assert_ne!(
        count_tokens(&o, probe),
        count_tokens(&c, probe),
        "gpt-4o and gpt-4 must resolve to distinct encodings"
    );

    let root = fixtures().join("scope/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();
    let r4o = count_budget(&closure, "gpt-4o", &Config::blocking(1_000_000)).unwrap();
    let r4 = count_budget(&closure, "gpt-4", &Config::blocking(1_000_000)).unwrap();
    // Reference total is identical regardless of target model.
    assert_eq!(r4o.o200k_tokens, r4.o200k_tokens);
    // The o200k target mirrors the reference total exactly.
    assert_eq!(r4o.target_tokens, r4o.o200k_tokens);
}

// Criterion 3: table-driven verdict (under -> Pass, over+default -> Block,
// over+warn-first -> Warn).
#[test]
fn verdict_table() {
    let root = fixtures().join("closure/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();
    let total = count_budget(&closure, "gpt-4o", &Config::blocking(1_000_000))
        .unwrap()
        .gating_target_tokens;
    assert!(total > 0);
    let over_ceiling = total - 1; // closure exceeds this
    let under_ceiling = total + 1; // closure fits under this

    struct Case {
        ceiling: usize,
        warn_first: bool,
        want: Verdict,
    }
    let cases = [
        Case {
            ceiling: under_ceiling,
            warn_first: false,
            want: Verdict::Pass,
        },
        Case {
            ceiling: under_ceiling,
            warn_first: true,
            want: Verdict::Pass,
        },
        Case {
            ceiling: over_ceiling,
            warn_first: false,
            want: Verdict::Block,
        },
        Case {
            ceiling: over_ceiling,
            warn_first: true,
            want: Verdict::Warn,
        },
    ];

    for c in cases {
        let cfg = Config {
            ceiling: c.ceiling,
            warn_first: c.warn_first,
            changed_files: None,
        };
        let report = count_budget(&closure, "gpt-4o", &cfg).unwrap();
        assert_eq!(
            report.verdict, c.want,
            "ceiling={} warn_first={}",
            c.ceiling, c.warn_first
        );
    }
}

// Criterion 4: inline suppression marker suppresses failure and captures reason.
#[test]
fn suppression_marker_suppresses_and_captures_reason() {
    let dir = scratch("suppress");
    copy_dir(&fixtures().join("suppress"), &dir);
    let root = dir.join("suppressed.md");
    let closure = resolve_closure(&root).unwrap();

    // Ceiling of 1 would Block any non-trivial file; suppression must rescue it.
    let report = count_budget(&closure, "gpt-4o", &Config::blocking(1)).unwrap();
    assert_eq!(
        report.verdict,
        Verdict::Pass,
        "suppressed file does not gate"
    );
    assert_eq!(
        report.gating_target_tokens, 0,
        "suppressed file excluded from gate"
    );

    let suppressions = report.suppressions();
    assert_eq!(suppressions.len(), 1);
    assert!(
        suppressions[0].1.contains("AOA-123"),
        "captured reason: {:?}",
        suppressions[0].1
    );
}

// Criterion 5: diff-scoped mode gates ONLY files in the provided changed list.
#[test]
fn diff_scope_gates_only_changed_files() {
    let root = fixtures().join("scope/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();

    let changed: PathBuf = closure
        .files
        .iter()
        .find(|f| f.path.file_name().unwrap() == "changed.md")
        .unwrap()
        .path
        .clone();
    let changed_tokens = {
        let full = count_budget(&closure, "gpt-4o", &Config::blocking(usize::MAX)).unwrap();
        full.files
            .iter()
            .find(|f| f.path == changed)
            .unwrap()
            .target_tokens
    };

    let mut set = BTreeSet::new();
    set.insert(changed.clone());
    let cfg = Config {
        ceiling: usize::MAX,
        warn_first: false,
        changed_files: Some(set),
    };
    let report = count_budget(&closure, "gpt-4o", &cfg).unwrap();

    // Only the changed file gates; the gating sum equals just its tokens.
    assert_eq!(report.gating_target_tokens, changed_tokens);
    let gating: Vec<_> = report.files.iter().filter(|f| f.gating).collect();
    assert_eq!(gating.len(), 1);
    assert_eq!(gating[0].path, changed);
    // The unchanged file is still reported, just not gating.
    assert!(report.target_tokens > report.gating_target_tokens);
}

// Criterion 6: fix reduces an over-budget file and re-check returns green.
#[test]
fn fix_oversized_file_rechecks_green() {
    let dir = scratch("fix");
    copy_dir(&fixtures().join("oversized"), &dir);
    let root = dir.join("big.md");

    let before = count_budget(
        &resolve_closure(&root).unwrap(),
        "gpt-4o",
        &Config::blocking(200),
    )
    .unwrap();
    assert_eq!(before.verdict, Verdict::Block, "fixture starts over budget");

    let outcome = fix_oversized(&root, 200, "gpt-4o").unwrap();
    assert!(outcome.archive.exists(), "full body archived");
    assert!(outcome.target_tokens < 200, "fix reported under ceiling");

    let after = count_budget(
        &resolve_closure(&root).unwrap(),
        "gpt-4o",
        &Config::blocking(200),
    )
    .unwrap();
    assert_eq!(after.verdict, Verdict::Pass, "closure green after fix");
    assert!(
        after.gating_target_tokens < 200,
        "post-fix closure tokens {} < ceiling 200",
        after.gating_target_tokens
    );
}

// Criterion 7: unknown target tokenizer fails loudly (Err, no silent default).
#[test]
fn unknown_target_tokenizer_errors() {
    let root = fixtures().join("closure/AGENTS.md");
    let closure = resolve_closure(&root).unwrap();
    let err = count_budget(&closure, "totally-made-up-model", &Config::blocking(100)).unwrap_err();
    assert!(matches!(err, BudgetError::UnknownTargetTokenizer { .. }));
}
