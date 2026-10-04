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

#[test]
fn angle_bracket_destinations_are_unwrapped_before_classification() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/angle/AGENTS.md");
    let report = lint_context(&root, "o200k_base").expect("lint_context should succeed");

    let stale: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.category == SmellCategory::StaleReference)
        .map(|f| f.message.as_str())
        .collect();

    assert_eq!(
        stale,
        ["stale reference: linked file 'docs/missing (old).md' does not exist"],
        "an angle-bracket URL is external and an existing angle-bracket path resolves; only the missing path is stale"
    );
}

fn mixed_report() -> LintReport {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mixed/AGENTS.md");
    lint_context(&root, "o200k_base").expect("lint_context should succeed")
}

fn findings_for<'a>(report: &'a LintReport, file_name: &str) -> Vec<&'a Finding> {
    report
        .findings
        .iter()
        .filter(|f| f.file.file_name().is_some_and(|name| name == file_name))
        .collect()
}

#[test]
fn section_rules_skip_non_markdown_closure_members() {
    let report = mixed_report();

    for file_name in ["snapshot.toml", "sync.ts", "deploy"] {
        assert!(
            report
                .budget
                .files
                .iter()
                .any(|f| f.path.file_name().is_some_and(|name| name == file_name)),
            "{file_name} should be a closure member"
        );
        let section_findings: Vec<&Finding> = findings_for(&report, file_name)
            .into_iter()
            .filter(|f| {
                matches!(
                    f.category,
                    SmellCategory::Verbosity | SmellCategory::Duplication
                )
            })
            .collect();
        assert!(
            section_findings.is_empty(),
            "{file_name} is not markdown, got section findings {section_findings:?}"
        );
    }
}

#[test]
fn section_rules_still_apply_to_extensionless_context_files() {
    let report = mixed_report();

    let duplicates: Vec<&Finding> = findings_for(&report, ".cursorrules")
        .into_iter()
        .filter(|f| f.category == SmellCategory::Duplication)
        .collect();
    assert_eq!(
        duplicates.len(),
        1,
        "expected the repeated heading in .cursorrules, got {duplicates:?}"
    );
}

#[test]
fn a_lone_nested_root_is_bounded_by_the_linted_directory_and_names_the_link_that_leaves_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    std::fs::write(outside.path().join("notes.md"), "elsewhere\n").unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/shared.md"), "shared\n").unwrap();
    std::fs::write(
        dir.path().join("pkg/AGENTS.md"),
        format!(
            "[shared](../docs/shared.md) [out]({})\n",
            outside.path().join("notes.md").display()
        ),
    )
    .unwrap();

    let report = aoa_lint::lint_context_roots(
        &[dir.path().join("pkg/AGENTS.md")],
        dir.path(),
        "o200k_base",
    )
    .unwrap();

    assert_eq!(
        report.closures[0].outside_boundary,
        [outside.path().join("notes.md")]
    );
    let nested: Vec<&Path> = report.closures[0]
        .files
        .iter()
        .map(|file| file.path.strip_prefix(dir.path()).unwrap())
        .collect();
    assert_eq!(
        nested,
        [Path::new("pkg/AGENTS.md"), Path::new("docs/shared.md")]
    );
}

#[cfg(unix)]
#[test]
fn a_link_through_a_directory_symlink_that_leaves_the_linted_directory_is_not_probed() {
    let dir = tempfile::TempDir::new().unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("docs")).unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "[through](docs/absent.md) [deeper](docs/absent/notes.md) [inside](absent.md)\n",
    )
    .unwrap();

    let report =
        aoa_lint::lint_context_roots(&[dir.path().join("AGENTS.md")], dir.path(), "o200k_base")
            .unwrap();

    let stale: Vec<&str> = report
        .findings
        .iter()
        .filter(|finding| finding.category == SmellCategory::StaleReference)
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(
        stale,
        ["stale reference: linked file 'absent.md' does not exist"]
    );
}

#[test]
fn no_prose_rule_runs_on_a_non_markdown_closure_member() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "[config](build.toml) [script](sync.ts)\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("build.toml"),
        "include = [ \"**\" ]\n# always run the formatter\n# never run the formatter\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("sync.ts"),
        "const next = handlers[index](gone.md);\n",
    )
    .unwrap();

    let report = lint_context(&dir.path().join("AGENTS.md"), "o200k_base").unwrap();

    assert_eq!(report.budget.files.len(), 3);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_missing_link_that_leaves_the_linted_directory_is_not_probed() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("repo")).unwrap();
    let root = dir.path().join("repo/AGENTS.md");
    std::fs::write(&root, "[out](../absent.md) [gone](absent.md)\n").unwrap();

    let report =
        aoa_lint::lint_context_roots(&[root], &dir.path().join("repo"), "o200k_base").unwrap();

    let stale: Vec<&str> = report
        .findings
        .iter()
        .filter(|finding| finding.category == SmellCategory::StaleReference)
        .map(|finding| finding.message.as_str())
        .collect();
    assert_eq!(
        stale,
        ["stale reference: linked file 'absent.md' does not exist"]
    );
}

#[cfg(unix)]
fn stale_references(repo: &std::path::Path, body: &str) -> Vec<String> {
    let root = repo.join("AGENTS.md");
    std::fs::write(&root, body).unwrap();
    aoa_lint::lint_context_roots(&[root], repo, "o200k_base")
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.category == SmellCategory::StaleReference)
        .map(|finding| finding.message)
        .collect()
}

#[cfg(unix)]
#[test]
fn a_dangling_link_that_names_a_path_outside_reads_the_same_whether_or_not_it_exists() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    symlink("../probed.md", repo.join("direct.md")).unwrap();
    symlink("hop.md", repo.join("chained.md")).unwrap();
    symlink("../probed.md", repo.join("hop.md")).unwrap();
    symlink("../probed-dir", repo.join("through")).unwrap();
    let body = "[direct](direct.md) [chained](chained.md) [through](through/member.md)\n";

    let while_absent = stale_references(&repo, body);
    std::fs::write(dir.path().join("probed.md"), "outside\n").unwrap();
    std::fs::create_dir(dir.path().join("probed-dir")).unwrap();
    let while_present = stale_references(&repo, body);

    assert_eq!(while_absent, Vec::<String>::new());
    assert_eq!(while_present, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_dangling_link_that_steps_through_a_path_outside_reads_the_same_whether_or_not_it_exists() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    symlink(
        "../../probed-dir/../repo/gone.md",
        repo.join("docs/through.md"),
    )
    .unwrap();
    let body = "[through](docs/through.md)\n";

    let while_absent = stale_references(&repo, body);
    std::fs::create_dir(dir.path().join("probed-dir")).unwrap();
    let while_present = stale_references(&repo, body);

    assert_eq!(while_absent, Vec::<String>::new());
    assert_eq!(while_present, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_long_chain_of_links_ending_outside_reads_the_same_whether_or_not_its_end_exists() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for hop in 0..45 {
        symlink(format!("h{}", hop + 1), repo.join(format!("h{hop}"))).unwrap();
    }
    symlink("../probed", repo.join("h45")).unwrap();
    let body = "[first](h0) [later](h4)\n";

    let while_absent = stale_references(&repo, body);
    std::fs::write(dir.path().join("probed"), "outside\n").unwrap();
    let while_present = stale_references(&repo, body);

    assert_eq!(while_absent, Vec::<String>::new());
    assert_eq!(while_present, Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_dangling_link_that_only_climbs_before_naming_a_path_inside_is_still_a_stale_reference() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("docs/deep")).unwrap();
    symlink("../gone.md", repo.join("docs/up.md")).unwrap();
    symlink("../../removed", repo.join("docs/deep/through")).unwrap();

    let stale = stale_references(
        &repo,
        "[up](docs/up.md) [through](docs/deep/through/member.md)\n",
    );

    assert_eq!(
        stale,
        [
            "stale reference: linked file 'docs/up.md' does not exist",
            "stale reference: linked file 'docs/deep/through/member.md' does not exist",
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_or_looping_link_that_stays_inside_is_still_a_stale_reference() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    symlink("removed.md", repo.join("gone.md")).unwrap();
    symlink("loop.md", repo.join("loop.md")).unwrap();

    let stale = stale_references(&repo, "[gone](gone.md) [loop](loop.md)\n");

    assert_eq!(
        stale,
        [
            "stale reference: linked file 'gone.md' does not exist",
            "stale reference: linked file 'loop.md' does not exist",
        ]
    );
}

#[test]
fn a_linked_build_file_is_counted_but_not_linted_as_prose() {
    let dir = tempfile::TempDir::new().unwrap();
    let step = "RUN make every single target in the tree and then some more\n";
    std::fs::write(
        dir.path().join("Dockerfile"),
        format!("{step}\n{step}\n{step}\n[gone](absent.md)\n"),
    )
    .unwrap();
    let root = dir.path().join("AGENTS.md");
    std::fs::write(&root, "[image](Dockerfile)\n").unwrap();

    let report = aoa_lint::lint_context_roots(&[root], dir.path(), "o200k_base").unwrap();

    assert_eq!(report.closures[0].files.len(), 2);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_member_that_cannot_be_counted_is_named_on_its_closure() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("binary.md"), b"rules\n\xff\n").unwrap();
    let root = dir.path().join("AGENTS.md");
    std::fs::write(&root, "[binary](binary.md)\n").unwrap();

    let report = aoa_lint::lint_context_roots(&[root], dir.path(), "o200k_base").unwrap();

    assert_eq!(
        report.closures[0].unread,
        [aoa_budget::UnreadLink {
            path: dir.path().join("binary.md"),
            reason: aoa_budget::UnreadReason::NotUtf8,
        }]
    );
}
