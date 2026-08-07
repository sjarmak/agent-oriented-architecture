//! The skip convention is a contract, not a comment (aoa-otb9t).
//!
//! Two ways exist in this workspace to make a test that needs something the
//! machine may not have stop early, and they are not interchangeable:
//!
//! - `eprintln!("SKIP …")` + `return` — the test still runs and reports `ok`.
//! - `#[ignore]` — the test does not run and the summary says `1 ignored`.
//!
//! Which one is honest turns on whether CI can ever satisfy the precondition.
//! `docs/adr/0004-environment-dependent-test-skips.md` decides that and records
//! why each site below is classified as it is; [`CI_CAN`] and [`CI_CANNOT`]
//! carry both halves into the failure messages, so an author who trips this file
//! is told the rule rather than sent to look it up.
//!
//! A rule nobody is forced to open is not a rule, which is what this file adds:
//! it scans the workspace for both idioms and fails when a site appears that the
//! ADR has not classified, so the third case is decided deliberately rather than
//! by whichever example its author happened to copy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The decision this test enforces, relative to the workspace root. Named in
/// every failure message, so it has to keep resolving.
const ADR: &str = "docs/adr/0004-environment-dependent-test-skips.md";

/// Sources carrying a printed SKIP notice, with why each is allowed to.
///
/// - `imports_python.rs` / `imports_typescript.rs` — `ruff`, `node`, and the
///   vendored ESLint are all installed by `.github/workflows/rust-ci.yml`, so
///   these run for real in CI and the notice is a local-dev affordance.
/// - `exposure_scan.rs` — the notice is nested *inside* an already-ignored test
///   and reports an unset `AOA_R0_CAMPAIGN_RUNS` to somebody who asked for the
///   test by name with `--ignored`. It is never the thing that decides whether a
///   green CI run means anything, because `#[ignore]` already decided that.
const PRINTED_NOTICE: &[&str] = &[
    "crates/aoa-bench/tests/exposure_scan.rs",
    "crates/aoa-migrate/tests/imports_python.rs",
    "crates/aoa-migrate/tests/imports_typescript.rs",
];

/// Sources holding a test that is ignored by default, with why each is.
///
/// - `exposure_scan.rs` — scans the real R0 campaign, which codeprobe produces
///   on an operator's machine. No CI runner can ever hold it.
const IGNORED_BY_DEFAULT: &[&str] = &["crates/aoa-bench/tests/exposure_scan.rs"];

fn workspace_root() -> PathBuf {
    // crates/aoa -> crates -> workspace root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/aoa sits two levels below the workspace root")
        .to_path_buf()
}

/// Every `.rs` file under `crates/`, workspace-relative and slash-separated.
///
/// Panics rather than returning an empty set when the walk finds nothing: a
/// silently empty listing would make both assertions below vacuous passes, which
/// is the failure this file exists to prevent rather than commit.
fn rust_sources() -> BTreeSet<String> {
    let root = workspace_root();
    let mut sources = BTreeSet::new();
    let mut pending = vec![root.join("crates")];

    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{} is readable: {e}", dir.display()));
        for entry in entries {
            let entry =
                entry.unwrap_or_else(|e| panic!("{} entry is readable: {e}", dir.display()));
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let file_type = entry
                .file_type()
                .unwrap_or_else(|e| panic!("{name} has a readable file type: {e}"));

            // `file_type` does not follow symlinks, so a symlinked directory is
            // neither descended into nor mistaken for a file — the walk cannot
            // cycle, and it cannot leave the tree it was pointed at.
            if file_type.is_dir() {
                // `target/` holds build output, and the vendored ESLint tree
                // holds JavaScript; neither is workspace source.
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                pending.push(entry.path());
            } else if file_type.is_file() && name.ends_with(".rs") {
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .expect("the walk started at the workspace root")
                    .to_string_lossy()
                    .replace('\\', "/");
                sources.insert(relative);
            }
        }
    }

    assert!(
        !sources.is_empty(),
        "no .rs files found under crates/, so every check in this file would pass \
         without reading a single source"
    );

    // This file quotes both markers in order to search for them, so leaving it
    // in the set would make it report itself forever. It is the classifier, not
    // a classified site: it holds no test with an environment precondition, and
    // if one is ever added here it belongs in a file that skips, not in the one
    // that decides what skipping means. Removing by `file!()` rather than by a
    // written-out name keeps the exclusion correct across a rename.
    let this_file = file!().replace('\\', "/");
    assert!(
        sources.remove(&this_file),
        "the walk did not reach {this_file}, so `file!()` no longer resolves the way \
         this exclusion assumes and the walk may be missing sources it should read"
    );

    sources
}

/// The subset of workspace sources whose text satisfies `matches`.
fn sources_matching(matches: fn(&str) -> bool) -> BTreeSet<String> {
    let root = workspace_root();
    rust_sources()
        .into_iter()
        .filter(|relative| {
            let source = std::fs::read_to_string(root.join(relative))
                .unwrap_or_else(|e| panic!("{relative} is readable: {e}"));
            matches(&source)
        })
        .collect()
}

/// A printed notice can sit anywhere on a line, so this matches the whole
/// source. The opening quote and the trailing space are both load-bearing: they
/// find `eprintln!("SKIP …")` and not the `SKIP_DIRS` walk constants in
/// `aoa-audit` and `aoa-migrate`.
fn prints_a_skip_notice(source: &str) -> bool {
    source.contains("\"SKIP ")
}

/// Matched without the closing bracket so both `#[ignore]` and `#[ignore = "…"]`
/// are found, and only at the start of a trimmed line — an attribute always opens
/// its line under rustfmt, so anchoring is what lets a doc comment discuss
/// `#[ignore]` without being counted as a use of it.
fn ignores_a_test(source: &str) -> bool {
    source
        .lines()
        .any(|line| line.trim_start().starts_with("#[ignore"))
}

/// Compare what the workspace does against what the ADR classified, reporting
/// each direction of drift as the different problem it is.
///
/// `convention` names the idiom that was found. Both halves of the rule are
/// printed either way, because the point of the message is to send an author who
/// copied the wrong nearby example to the other branch of the question.
fn assert_classified(matches: fn(&str) -> bool, registered: &[&str], convention: &str) {
    let found = sources_matching(matches);
    let registered: BTreeSet<String> = registered.iter().map(|s| (*s).to_owned()).collect();

    let unclassified: Vec<&String> = found.difference(&registered).collect();
    assert!(
        unclassified.is_empty(),
        "these sources skip via {convention} but {ADR} has not classified them: \
         {unclassified:?}\n\
         The question that decides the convention is: can CI ever satisfy this \
         precondition?\n  \
         Yes -> {CI_CAN}\n  \
         No  -> {CI_CANNOT}\n\
         Answer it in {ADR}, then add the source to this test's registry."
    );

    let stale: Vec<&String> = registered.difference(&found).collect();
    assert!(
        stale.is_empty(),
        "{ADR} classifies these sources as skipping via {convention}, but they no \
         longer do: {stale:?}\n\
         Drop them from the record and from this test's registry so the decision \
         describes the workspace as it stands."
    );
}

/// The two halves of the rule, written once so the two checks cannot drift into
/// disagreeing about what the decision was.
const CI_CAN: &str = "eprintln!(\"SKIP …\") + return, and CI must install whatever the \
                      test needs so the skip stays a local-dev affordance.";
const CI_CANNOT: &str = "#[ignore], because libtest captures a passing test's output, so a \
                         printed notice would report `ok` for a run that checked nothing.";

#[test]
fn the_decision_record_this_test_cites_exists() {
    let path = workspace_root().join(ADR);
    assert!(
        path.is_file(),
        "{ADR} does not exist, so every failure message in this file points a test \
         author at nothing"
    );
}

#[test]
fn every_printed_skip_notice_is_classified() {
    assert_classified(
        prints_a_skip_notice,
        PRINTED_NOTICE,
        "a printed SKIP notice",
    );
}

#[test]
fn every_ignored_test_is_classified() {
    assert_classified(
        ignores_a_test,
        IGNORED_BY_DEFAULT,
        "an ignored-by-default attribute",
    );
}
