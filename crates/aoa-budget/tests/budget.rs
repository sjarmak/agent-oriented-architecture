use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aoa_budget::{
    count_budget, extract_references, fix_oversized, resolve_closure, BudgetError, Config, Verdict,
    REFERENCE_ENCODING,
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

    let outcome = fix_oversized(&root, &dir, 200, "gpt-4o").unwrap();
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

#[test]
fn angle_bracket_destinations_are_unwrapped_before_classification() {
    let base = fixtures().join("angle");
    let text = std::fs::read_to_string(base.join("AGENTS.md")).unwrap();

    let targets: Vec<PathBuf> = extract_references(&text, &base)
        .into_iter()
        .map(|r| r.target)
        .collect();
    assert_eq!(
        targets,
        [
            base.join("docs/present (v2).md"),
            base.join("docs/missing (old).md"),
        ],
        "the angle-bracket URL is external; both angle-bracket paths are local"
    );

    let closure = resolve_closure(&base.join("AGENTS.md")).unwrap();
    let names: Vec<String> = closure
        .files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["AGENTS.md", "present (v2).md"]);
}

fn oversized_body() -> String {
    "A paragraph of guidance that every package must follow.\n\n".repeat(200)
}

fn names_in(dir: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    names
}

fn is_a_summary(path: &Path) -> bool {
    std::fs::symlink_metadata(path).unwrap().is_file()
        && std::fs::read_to_string(path)
            .unwrap()
            .starts_with("> Summarized to fit budget.")
}

fn is_its_own_file_holding(path: &Path, body: &str) -> bool {
    std::fs::symlink_metadata(path).unwrap().is_file()
        && std::fs::read_to_string(path).unwrap() == body
}

#[test]
fn fix_refuses_a_root_outside_the_boundary_and_leaves_it_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    std::fs::create_dir_all(dir.path().join("repo")).unwrap();
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    let root = dir.path().join("outside/big.md");
    std::fs::write(&root, &body).unwrap();

    let err = fix_oversized(&root, &dir.path().join("repo"), 200, "gpt-4o").unwrap_err();

    assert!(matches!(err, BudgetError::OutsideBoundary { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert!(!dir.path().join("outside/big.archive.md").exists());
}

#[cfg(unix)]
#[test]
fn fix_refuses_an_archive_linked_outside_the_boundary_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, &body).unwrap();
    let planted = dir.path().join("planted.md");
    std::fs::write(&planted, "planted\n").unwrap();
    std::os::unix::fs::symlink(&planted, repo.join("big.archive.md")).unwrap();
    let dangling = dir.path().join("created.md");
    let other = repo.join("other.md");
    std::fs::write(&other, &body).unwrap();
    std::os::unix::fs::symlink(&dangling, repo.join("other.archive.md")).unwrap();

    let linked = fix_oversized(&root, &repo, 200, "gpt-4o").unwrap_err();
    let unresolved = fix_oversized(&other, &repo, 200, "gpt-4o").unwrap_err();

    assert!(
        matches!(linked, BudgetError::OutsideBoundary { .. }),
        "{linked}"
    );
    assert!(
        matches!(unresolved, BudgetError::OutsideBoundary { .. }),
        "{unresolved}"
    );
    assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted\n");
    assert!(!dangling.exists());
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(std::fs::read_to_string(&other).unwrap(), body);
}

#[cfg(unix)]
#[test]
fn fix_refuses_an_archive_linked_out_of_the_boundary_and_back_in_whatever_it_steps_through() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, &body).unwrap();
    std::fs::write(repo.join("kept.md"), "kept\n").unwrap();
    std::os::unix::fs::symlink("../probed-dir/../repo/kept.md", repo.join("big.archive.md"))
        .unwrap();
    let refused = || {
        let err = fix_oversized(&root, &repo, 200, "gpt-4o").unwrap_err();
        matches!(err, BudgetError::OutsideBoundary { .. })
    };

    let while_absent = refused();
    std::fs::create_dir(dir.path().join("probed-dir")).unwrap();
    let while_a_directory = refused();

    assert!(while_absent && while_a_directory);
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(
        std::fs::read_to_string(repo.join("kept.md")).unwrap(),
        "kept\n"
    );
}

#[cfg(unix)]
#[test]
fn fix_replaces_an_archive_that_is_a_link_inside_the_boundary_and_leaves_its_target_alone() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("hooks")).unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, &body).unwrap();
    std::fs::write(repo.join("kept.md"), "kept\n").unwrap();
    std::os::unix::fs::symlink("kept.md", repo.join("big.archive.md")).unwrap();
    let other = repo.join("other.md");
    std::fs::write(&other, &body).unwrap();
    std::os::unix::fs::symlink("hooks/created", repo.join("other.archive.md")).unwrap();

    let linked = fix_oversized(&root, &repo, 200, "gpt-4o");
    let dangling = fix_oversized(&other, &repo, 200, "gpt-4o");

    assert_eq!(
        std::fs::read_to_string(repo.join("kept.md")).unwrap(),
        "kept\n"
    );
    assert!(!repo.join("hooks/created").exists());
    assert!(linked.is_ok(), "{linked:?}");
    assert!(dangling.is_ok(), "{dangling:?}");
    assert!(is_its_own_file_holding(&repo.join("big.archive.md"), &body));
    assert!(is_its_own_file_holding(
        &repo.join("other.archive.md"),
        &body
    ));
    assert!(is_a_summary(&root) && is_a_summary(&other));
}

#[cfg(unix)]
#[test]
fn fix_refuses_a_root_reached_through_a_link_from_outside_the_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    std::fs::write(repo.join("big.md"), &body).unwrap();
    let alias = dir.path().join("outside/alias.md");
    std::os::unix::fs::symlink(repo.join("big.md"), &alias).unwrap();

    let err = fix_oversized(&alias, &repo, 200, "gpt-4o").unwrap_err();

    assert!(matches!(err, BudgetError::OutsideBoundary { .. }), "{err}");
    assert_eq!(std::fs::read_to_string(repo.join("big.md")).unwrap(), body);
    assert!(!dir.path().join("outside/alias.archive.md").exists());
}

#[cfg(unix)]
#[test]
fn fix_rewrites_the_file_it_read_when_the_root_steps_back_out_of_a_linked_directory() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("a/b")).unwrap();
    std::fs::write(repo.join("x.md"), &body).unwrap();
    std::fs::write(repo.join("a/x.md"), "kept\n").unwrap();
    std::os::unix::fs::symlink(repo.join("a/b"), repo.join("link")).unwrap();

    let fixed = fix_oversized(&repo.join("link/../x.md"), &repo, 200, "gpt-4o").unwrap();

    assert_eq!(fixed.root, repo.join("x.md"));
    assert_eq!(fixed.archive, repo.join("x.archive.md"));
    assert_eq!(std::fs::read_to_string(&fixed.archive).unwrap(), body);
    assert_ne!(std::fs::read_to_string(repo.join("x.md")).unwrap(), body);
    assert_eq!(
        std::fs::read_to_string(repo.join("a/x.md")).unwrap(),
        "kept\n"
    );
    assert!(!repo.join("a/x.archive.md").exists());
}

#[cfg(unix)]
#[test]
fn fix_leaves_a_file_outside_untouched_when_the_root_steps_back_onto_a_link_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("real/sub")).unwrap();
    std::fs::write(repo.join("doc.md"), &body).unwrap();
    let planted = dir.path().join("planted.md");
    std::fs::write(&planted, "planted\n").unwrap();
    std::os::unix::fs::symlink(repo.join("real/sub"), repo.join("docs")).unwrap();
    std::os::unix::fs::symlink(&planted, repo.join("real/doc.md")).unwrap();

    let fixed = fix_oversized(&repo.join("docs/../doc.md"), &repo, 200, "gpt-4o");

    assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted\n");
    assert!(!repo.join("real/doc.archive.md").exists());
    assert_eq!(fixed.unwrap().root, repo.join("doc.md"));
}

#[test]
fn fix_replaces_an_archive_that_is_a_hard_link_to_a_file_outside_the_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, &body).unwrap();
    let planted = dir.path().join("planted.md");
    std::fs::write(&planted, "planted\n").unwrap();
    std::fs::hard_link(&planted, repo.join("big.archive.md")).unwrap();

    let fixed = fix_oversized(&root, &repo, 200, "gpt-4o");

    assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted\n");
    assert!(fixed.is_ok(), "{fixed:?}");
    assert!(is_its_own_file_holding(&repo.join("big.archive.md"), &body));
    assert!(is_a_summary(&root));
}

#[test]
fn fix_replaces_a_root_that_is_a_hard_link_to_a_file_outside_the_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let planted = dir.path().join("planted.md");
    std::fs::write(&planted, &body).unwrap();
    let root = repo.join("big.md");
    std::fs::hard_link(&planted, &root).unwrap();

    let fixed = fix_oversized(&root, &repo, 200, "gpt-4o");

    assert_eq!(std::fs::read_to_string(&planted).unwrap(), body);
    assert!(fixed.is_ok(), "{fixed:?}");
    assert!(is_a_summary(&root));
    assert!(is_its_own_file_holding(&repo.join("big.archive.md"), &body));
}

#[cfg(unix)]
#[test]
fn fix_summarizes_the_file_a_linked_root_names_and_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("real.md"), &body).unwrap();
    let root = repo.join("big.md");
    std::os::unix::fs::symlink("real.md", &root).unwrap();

    let fixed = fix_oversized(&root, &repo, 200, "gpt-4o");

    assert!(fixed.is_ok(), "{fixed:?}");
    assert_eq!(std::fs::read_link(&root).unwrap(), Path::new("real.md"));
    assert!(is_a_summary(&repo.join("real.md")));
    assert!(is_its_own_file_holding(&repo.join("big.archive.md"), &body));
    assert_eq!(names_in(&repo), ["big.archive.md", "big.md", "real.md"]);
}

#[cfg(unix)]
#[test]
fn fix_replaces_an_archive_name_caught_in_a_link_loop_and_leaves_nothing_else_behind() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, &body).unwrap();
    let archive = repo.join("big.archive.md");
    std::os::unix::fs::symlink("big.archive.md", &archive).unwrap();

    let fixed = fix_oversized(&root, &repo, 200, "gpt-4o");

    assert!(fixed.is_ok(), "{fixed:?}");
    assert!(is_its_own_file_holding(&archive, &body));
    assert!(is_a_summary(&root));
    assert_eq!(names_in(&repo), ["big.archive.md", "big.md"]);
}

#[test]
fn fix_that_cannot_place_the_archive_changes_nothing_and_leaves_no_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let body = oversized_body();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("big.archive.md")).unwrap();
    std::fs::write(repo.join("big.archive.md/kept.md"), "kept\n").unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, &body).unwrap();

    let refused = fix_oversized(&root, &repo, 200, "gpt-4o");

    assert!(
        matches!(refused, Err(BudgetError::Io { .. })),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&root).unwrap(), body);
    assert_eq!(names_in(&repo), ["big.archive.md", "big.md"]);
    assert_eq!(
        std::fs::read_to_string(repo.join("big.archive.md/kept.md")).unwrap(),
        "kept\n"
    );
}

#[cfg(unix)]
#[test]
fn fix_keeps_the_permissions_of_the_root_on_both_files_it_writes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let root = repo.join("big.md");
    std::fs::write(&root, oversized_body()).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o640)).unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;

    let fixed = fix_oversized(&root, &repo, 200, "gpt-4o").unwrap();

    assert_eq!(mode(&fixed.root), 0o640);
    assert_eq!(mode(&fixed.archive), 0o640);
}

#[test]
fn fix_recheck_counts_what_the_root_links_in_a_sibling_directory() {
    let dir = scratch("fix-sibling");
    let body = "A paragraph of shared guidance that every package must follow.\n\n".repeat(200);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::write(dir.join("docs/shared.md"), &body).unwrap();
    let root = dir.join("pkg/AGENTS.md");
    std::fs::write(
        &root,
        format!("# Package\n\n[shared](../docs/shared.md)\n\n{body}"),
    )
    .unwrap();

    let err = fix_oversized(&root, &dir, 200, "gpt-4o").unwrap_err();

    assert!(matches!(err, BudgetError::FixFailed { .. }), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}
