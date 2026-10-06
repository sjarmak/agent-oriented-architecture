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
//! A rule nobody is forced to open is not a rule, which is what this file adds.
//! It holds the four things the record needs to stay true, each of which decays
//! silently on its own:
//!
//! 1. The workspace agrees with the registry — counted per site, not per file.
//!    A file blessed once would otherwise pre-approve every later skip added
//!    anywhere inside it.
//! 2. The registry agrees with the record — the ADR names every source it
//!    classifies, so the registry cannot grow entries the decision never made.
//! 3. The record is reachable from where it applies — CLAUDE.md and every
//!    classified source cite a path that resolves.
//! 4. CI still installs what the printed-notice sites depend on. That install is
//!    the entire reason those sites are allowed to be invisible; delete it and
//!    they become the case the record forbids, silently.
//!
//! ## What this does not catch
//!
//! The scan is textual, so its reach is the two idioms spelled the documented
//! way. A test that returns early with no notice at all is invisible here, and
//! so is an attribute a macro emits. Neither is a gap to close by widening the
//! needles until they guess: a site that announces nothing is a defect in that
//! test rather than an unclassified convention, and the honest statement of this
//! file's guarantee is "no *detectable* skip site is unclassified".
//!
//! Widening [`count_sites`] trades false negatives for false positives, and the
//! two are not symmetric. A missed site is a green CI run that checked nothing —
//! the failure the ADR exists to prevent. A spurious match is a red workspace
//! gate, loud and cheap, whose only real danger is that somebody quiets it by
//! adding a fake row to [`CLASSIFIED`] and corrupting the record. [`NOT_A_SKIP`]
//! exists so that has a correct answer instead.

use std::collections::{BTreeMap, BTreeSet};

mod common;

use common::{read, workspace_root};

/// The decision this test enforces, relative to the workspace root. Named in
/// every failure message and cited from CLAUDE.md and each classified source, so
/// it has to keep resolving.
const ADR: &str = "docs/adr/0004-environment-dependent-test-skips.md";

/// The workflow whose installs keep the printed-notice sites on the legal side
/// of the rule.
const CI_WORKFLOW: &str = ".github/workflows/rust-ci.yml";

/// One classified source and how many sites of each convention the record
/// accounts for. Counts rather than a bare path, so a skip added to an
/// already-classified file has to be classified too.
struct Classified {
    path: &'static str,
    /// Printed `SKIP` notices, legal because CI satisfies the precondition.
    notices: usize,
    /// Tests ignored by default, because CI never can.
    ignored: usize,
}

/// Every environment-dependent skip site in the workspace, as ADR 0004
/// classifies it.
///
/// - `exposure_scan.rs` — the ignored test scans the real R0 campaign, which
///   codeprobe produces on an operator's machine and no runner can hold. Its one
///   notice is nested *inside* that ignored test and reports an unset
///   `AOA_R0_CAMPAIGN_RUNS` to somebody who asked for it by name with
///   `--ignored`; `#[ignore]` already made the CI-honesty decision.
/// - `imports_python.rs` / `imports_typescript.rs` — `ruff`, `node`, and the
///   vendored ESLint are installed by [`CI_WORKFLOW`] in both its jobs, so these
///   run for real in CI and the notices are a local-dev affordance. TypeScript
///   carries two because it probes for `node` and the vendored ESLint
///   separately.
const CLASSIFIED: &[Classified] = &[
    Classified {
        path: "crates/aoa-audit/tests/context_root_containment.rs",
        notices: 1,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-bench/tests/exposure_scan.rs",
        notices: 1,
        ignored: 1,
    },
    Classified {
        path: "crates/aoa-budget/tests/closure_bounds.rs",
        notices: 3,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-budget/tests/fix_replace.rs",
        notices: 2,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-codeprobe-shim/tests/trace_db.rs",
        notices: 1,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-enforce/src/live_log/open.rs",
        notices: 1,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-lint/tests/lint.rs",
        notices: 1,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-migrate/tests/imports_python.rs",
        notices: 1,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-migrate/tests/imports_typescript.rs",
        notices: 2,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-path-trust/src/nofollow.rs",
        notices: 2,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-path-trust/src/root.rs",
        notices: 1,
        ignored: 0,
    },
    Classified {
        path: "crates/aoa-scip-graph/tests/walk_scope.rs",
        notices: 1,
        ignored: 0,
    },
];

/// Sources the scan matches that hold no skip site at all — a doc comment
/// quoting the convention, or a Rust fixture in a raw string whose text happens
/// to carry an attribute.
///
/// Empty, which is the state it is meant to be in. This is the same shape as
/// `architecture_doc.rs`'s `LAYER_EXCEPTIONS`, and carries the same warning: an
/// entry is a licence for the scan to be wrong about one file, not a place to
/// park a skip site somebody did not want to classify. Anything listed here must
/// genuinely not skip on an environment precondition — if it does, it belongs in
/// [`CLASSIFIED`] with the reason written into the ADR.
const NOT_A_SKIP: &[&str] = &[];

/// The two halves of the rule, written once so the checks cannot drift into
/// disagreeing about what the decision was.
const CI_CAN: &str = "eprintln!(\"SKIP …\") + return, and CI must install whatever the \
                      test needs so the skip stays a local-dev affordance.";
const CI_CANNOT: &str = "#[ignore], because libtest captures a passing test's output, so a \
                         printed notice would report `ok` for a run that checked nothing.";

/// How many sites of each convention a source holds.
#[derive(Default, PartialEq, Eq)]
struct Sites {
    notices: usize,
    ignored: usize,
}

/// Count both conventions in one pass.
///
/// The notice needle is `"SKIP` — an opening quote and the word, without the
/// trailing space an earlier version required. That space exempted
/// `eprintln!("SKIP: …")`, the same idiom with different punctuation. Dropping it
/// also matches a literal opening `SKIPPED`, which costs one [`NOT_A_SKIP`] row
/// if it ever occurs: the cheap direction to be wrong in.
///
/// An attribute opens its line under rustfmt, so anchoring `#[ignore` there
/// keeps prose that discusses it from being counted. `#[cfg_attr(…, ignore)]` is
/// the standard conditional form and is counted too, on one line, which is where
/// rustfmt leaves it.
fn count_sites(source: &str) -> Sites {
    let ignored = source
        .lines()
        .map(str::trim_start)
        .filter(|line| {
            line.starts_with("#[ignore")
                || (line.starts_with("#[cfg_attr") && line.contains("ignore"))
        })
        .count();

    Sites {
        notices: source.matches("\"SKIP").count(),
        ignored,
    }
}

/// Every `.rs` file under `crates/` holding at least one skip site, mapped to
/// its counts.
///
/// `crates/` is the whole workspace: `Cargo.toml` declares
/// `members = ["crates/*"]`, and the sibling `architecture_doc.rs` rests on the
/// same fact. A member added elsewhere would escape this walk and would also
/// break that test's crate-layer list, so the two fail together rather than one
/// of them going quietly wrong.
///
/// Panics rather than returning an empty map when the walk finds nothing: a
/// silently empty listing would make every assertion below a vacuous pass, which
/// is the failure this file exists to prevent rather than commit.
fn skip_sites() -> BTreeMap<String, Sites> {
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

    // This file quotes both needles in order to search for them, so leaving it
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
        .into_iter()
        .filter(|relative| !NOT_A_SKIP.contains(&relative.as_str()))
        .filter_map(|relative| {
            let sites = count_sites(&read(&relative));
            (sites != Sites::default()).then_some((relative, sites))
        })
        .collect()
}

#[test]
fn every_skip_site_is_classified() {
    let found = skip_sites();
    let registered: BTreeMap<&str, Sites> = CLASSIFIED
        .iter()
        .map(|c| {
            (
                c.path,
                Sites {
                    notices: c.notices,
                    ignored: c.ignored,
                },
            )
        })
        .collect();

    let unclassified: Vec<String> = found
        .iter()
        .filter(|(path, sites)| registered.get(path.as_str()) != Some(sites))
        .map(|(path, sites)| {
            let (n, i) = registered
                .get(path.as_str())
                .map_or((0, 0), |r| (r.notices, r.ignored));
            format!(
                "{path}: found {} notice(s) and {} ignored; {ADR} accounts for {n} and {i}",
                sites.notices, sites.ignored
            )
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "the workspace holds skip sites {ADR} has not classified:\n  {}\n\
         The question that decides the convention is: can CI ever satisfy this \
         precondition?\n  \
         Yes -> {CI_CAN}\n  \
         No  -> {CI_CANNOT}\n\
         Answer it in {ADR}, then update this file's CLASSIFIED counts.\n\
         If the match is not a skip site at all — a comment quoting the \
         convention, or an attribute inside a raw-string fixture — it belongs in \
         NOT_A_SKIP, never in CLASSIFIED.",
        unclassified.join("\n  ")
    );

    let stale: Vec<&str> = registered
        .keys()
        .filter(|path| !found.contains_key(**path))
        .copied()
        .collect();
    assert!(
        stale.is_empty(),
        "{ADR} classifies these sources but they no longer skip on anything: \
         {stale:?}\n\
         Drop them from the record and from CLASSIFIED so the decision describes \
         the workspace as it stands."
    );
}

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
fn the_decision_record_names_every_classified_source() {
    let adr = read(ADR);
    // By file name, not full path: the record groups the two aoa-migrate suites
    // and names the second one alone, which is how a human writes it.
    let unmentioned: Vec<&str> = CLASSIFIED
        .iter()
        .map(|c| c.path)
        .filter(|path| {
            let name = path.rsplit('/').next().expect("a path has a last segment");
            !adr.contains(name)
        })
        .collect();

    assert!(
        unmentioned.is_empty(),
        "CLASSIFIED registers sources {ADR} never mentions: {unmentioned:?}\n\
         An entry with no matching sentence in the record is a classification \
         nobody made, and this test would then go green on a decision that was \
         never written down. Record why each is classified as it is, or drop it."
    );
}

#[test]
fn every_classified_source_cites_the_decision_record() {
    let uncited: Vec<&str> = CLASSIFIED
        .iter()
        .map(|c| c.path)
        .filter(|path| !read(path).contains(ADR))
        .collect();

    assert!(
        uncited.is_empty(),
        "these classified sources do not cite {ADR}: {uncited:?}\n\
         The author reading the skip site is the one who needs the rule, so each \
         site names the record that justifies it."
    );
}

#[test]
fn claude_md_records_the_rule() {
    let claude_md = read("CLAUDE.md");
    for path in [ADR, file!()] {
        assert!(
            claude_md.contains(path),
            "CLAUDE.md no longer names {path}, so its conventions section points a \
             test author at a path that does not resolve"
        );
    }
}

#[test]
fn ci_installs_what_the_printed_notices_depend_on() {
    let workflow = read(CI_WORKFLOW);
    // Presence, not shape. This catches the deletion the record calls fatal and
    // claims nothing about a subtler weakening (a version downgrade, a job that
    // stops running) — those are legible in a workflow diff, whereas a deleted
    // step is exactly what is not, because every other test here still passes.
    for (marker, tool) in [
        ("actions/setup-node", "node"),
        ("crates/aoa-migrate/assets/eslint", "the vendored ESLint"),
        ("pipx install ruff", "ruff"),
    ] {
        assert!(
            workflow.contains(marker),
            "{CI_WORKFLOW} no longer installs {tool} ({marker} is gone), so the \
             sites {ADR} classifies as printed notices would skip in CI and report \
             `ok` having checked nothing — the case the record forbids.\n\
             Restore the install, or reclassify those sites: {CI_CANNOT}"
        );
    }
}
