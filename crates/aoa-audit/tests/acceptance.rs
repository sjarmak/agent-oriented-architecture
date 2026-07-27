use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use aoa_audit::{audit, exit_code, observe, write_trace, AuditConfig, AuditReport, Tier};
use aoa_construct::MIN_HELD_OUT_OBSERVATIONS;
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
        node_paths: BTreeMap::new(),
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

    // The written file must literally carry the wire-format version — a symmetric
    // round-trip would pass even if both ends silently omitted it.
    let on_disk = std::fs::read_to_string(&path).expect("read written trace");
    assert!(
        on_disk.contains(&format!("\"version\": {}", aoa_trace::TRACE_FORMAT_VERSION)),
        "write_trace must stamp the wire-format version: {on_disk}"
    );
}

// Criterion (aoa-kk6m): write_trace refuses any caller-supplied name that could
// escape the installed .aoa/traces boundary — absolute, parent, dot, and
// multi-component names are rejected before any write, and no stray file lands
// in the repo.
#[test]
fn write_trace_rejects_escaping_names() {
    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");
    let before = file_set(repo.path());

    for name in [
        "/etc/passwd-aoa-should-never-write",
        "../../escape.json",
        "..",
        ".",
        "a/b.json",
        "nul\0name.json",
        "",
    ] {
        let err = write_trace(&outcome, name, &valid_trace())
            .expect_err(&format!("write_trace must reject {name:?}"));
        assert!(
            matches!(err, aoa_audit::AuditError::UnsafeTraceName { .. }),
            "wrong error for {name:?}: {err:?}"
        );
    }

    // Every name was rejected at validation, so no file was created in the tree.
    let after = file_set(repo.path());
    assert_eq!(before, after, "a rejected trace write must create no file");
}

// Criterion (aoa-8tw8): the append-only live-log lane is defended by the writer
// itself, not by caller discipline. `live-<session>.jsonl` files are extended by
// the enforce hooks under their own write lock; write_trace runs entirely
// outside that lock, so it must refuse those names rather than truncate a log
// mid-session.
#[test]
fn write_trace_refuses_to_clobber_an_active_live_log() {
    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");

    let live = outcome.traces_dir.join("live-s1.jsonl");
    std::fs::write(&live, "{\"span\":1}\n").expect("seed live log");

    let err = write_trace(&outcome, "live-s1.jsonl", &valid_trace())
        .expect_err("write_trace must reject a live-log name");
    assert!(
        matches!(err, aoa_audit::AuditError::UnsafeTraceName { .. }),
        "wrong error: {err:?}"
    );

    assert_eq!(
        std::fs::read_to_string(&live).expect("read live log"),
        "{\"span\":1}\n",
        "write_trace truncated an active live log"
    );
}

// Criterion (aoa-8tw8): a whole trace is a finished artifact. A second write to
// the same name is a collision, not an update — it must be refused, leaving the
// landed trace byte-identical.
#[test]
fn write_trace_refuses_to_overwrite_a_landed_trace() {
    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");

    let (path, _) = write_trace(&outcome, "run-1.json", &valid_trace()).expect("first write lands");
    let landed = std::fs::read_to_string(&path).expect("read landed trace");

    let err = write_trace(&outcome, "run-1.json", &valid_trace())
        .expect_err("write_trace must refuse to overwrite");
    assert!(
        matches!(err, aoa_audit::AuditError::TraceExists { .. }),
        "wrong error: {err:?}"
    );

    assert_eq!(
        std::fs::read_to_string(&path).expect("re-read landed trace"),
        landed,
        "the landed trace was modified by a refused write"
    );
}

// Criterion (aoa-kk6m): the symlink boundary is defended. A legal-looking name
// that is actually a symlink out of the trace dir must NOT be written through —
// the file it points at stays untouched, proving no write escapes.
#[cfg(unix)]
#[test]
fn write_trace_refuses_to_follow_a_symlink_out_of_the_trace_dir() {
    use std::os::unix::fs::symlink;

    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");

    // A file OUTSIDE the trace dir that must never be modified.
    let secret = repo.path().join("secret.txt");
    std::fs::write(&secret, "original").expect("seed secret");

    // Plant a symlink INSIDE the trace dir whose name looks legal but points at
    // the secret. Following it on write would clobber the secret.
    let link = outcome.traces_dir.join("escape.json");
    symlink(&secret, &link).expect("create symlink");

    let err = write_trace(&outcome, "escape.json", &valid_trace())
        .expect_err("write_trace must refuse to write through a symlink");
    assert!(
        matches!(err, aoa_audit::AuditError::UnsafeTraceName { .. }),
        "wrong error: {err:?}"
    );

    assert_eq!(
        std::fs::read_to_string(&secret).expect("read secret"),
        "original",
        "write escaped the trace dir through the symlink"
    );
}

// Criterion (aoa-vpgx): an ordinary-looking file can still share its inode
// with a file outside the trace directory. Whole traces are create-only, so a
// planted hardlink must be treated as a collision and its peer left untouched.
#[cfg(unix)]
#[test]
fn write_trace_refuses_to_overwrite_a_hardlinked_file() {
    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");
    let victim = repo.path().join("tracked.txt");
    std::fs::write(&victim, "original").expect("seed victim");
    std::fs::hard_link(&victim, outcome.traces_dir.join("escape.json")).expect("plant hardlink");

    let err = write_trace(&outcome, "escape.json", &valid_trace())
        .expect_err("write_trace must refuse an existing hardlink");
    assert!(
        matches!(err, aoa_audit::AuditError::TraceExists { .. }),
        "wrong error: {err:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&victim).expect("read victim"),
        "original",
        "write_trace modified the hardlink peer"
    );
}

#[cfg(unix)]
#[test]
fn write_trace_refuses_a_fifo_without_hanging() {
    let repo = fixture_repo();
    let outcome = observe(repo.path()).expect("observe succeeds");
    let fifo = outcome.traces_dir.join("blocked.json");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo is available");
    assert!(made.success(), "mkfifo failed");

    let err = write_trace(&outcome, "blocked.json", &valid_trace())
        .expect_err("write_trace must refuse a FIFO");
    assert!(
        matches!(err, aoa_audit::AuditError::TraceExists { .. }),
        "wrong error: {err:?}"
    );
}

// Criterion (aoa-pbqk): the INSTALL path is defended, not just the caller-
// supplied trace name. `create_dir_all` and `fs::write` both follow symlinks, so
// a `.aoa`, `.aoa/traces`, or `.aoa/.gitignore` planted as a link before install
// would relocate every subsequent write outside the repo while each trace name
// still passes the single-component check.
//
// The outside target is a REAL, EXISTING directory holding a seeded file. That
// distinction is what makes these tests genuinely red before the fix: pointed at
// a non-existent target, `create_dir_all` fails on its own with `Io` and a bare
// `expect_err` would pass without any guard.
#[cfg(unix)]
mod install_path_symlinks {
    use super::*;
    use std::os::unix::fs::symlink;

    /// An existing directory outside the repo, holding one seeded file, that no
    /// install may write into or through.
    fn outside_dir() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("create outside dir");
        let seeded = dir.path().join("seeded.txt");
        std::fs::write(&seeded, "original").expect("seed outside file");
        (dir, seeded)
    }

    /// Assert the outside tree is byte-for-byte what it was before the install
    /// attempt: same entries, same seeded content. Deliberately NOT expressed via
    /// `file_set` on the repo — that walker recurses through `path.is_dir()`,
    /// which FOLLOWS symlinks, so an escaped write would surface as an innocent
    /// `.aoa/traces/x` entry and the assertion would pass vacuously.
    fn assert_outside_untouched(outside: &Path, seeded: &Path) {
        let entries: BTreeSet<PathBuf> = std::fs::read_dir(outside)
            .expect("read outside dir")
            .flatten()
            .map(|e| e.file_name().into())
            .collect();
        assert_eq!(
            entries,
            BTreeSet::from([PathBuf::from("seeded.txt")]),
            "install wrote into the symlink target at {}",
            outside.display()
        );
        assert_eq!(
            std::fs::read_to_string(seeded).expect("read seeded file"),
            "original",
            "install wrote through the symlink and clobbered {}",
            seeded.display()
        );
    }

    /// Pin the reported node, not just the variant. Matching `{ .. }` alone
    /// would pass for a guard that fired on the wrong ancestor — which is
    /// precisely the regression the innermost-first walk could introduce.
    fn assert_unsafe_install_path(err: aoa_audit::AuditError, planted: &Path) {
        match err {
            aoa_audit::AuditError::UnsafeInstallPath { path } => {
                assert_eq!(path, planted, "guard fired on the wrong node")
            }
            other => panic!(
                "wrong error for planted symlink {}: {other:?}",
                planted.display()
            ),
        }
    }

    #[test]
    fn observe_refuses_when_dot_aoa_is_a_symlink() {
        let repo = fixture_repo();
        let (outside, seeded) = outside_dir();

        let planted = repo.path().join(".aoa");
        symlink(outside.path(), &planted).expect("plant .aoa symlink");

        let err = observe(repo.path()).expect_err("observe must refuse a symlinked .aoa");
        assert_unsafe_install_path(err, &planted);
        assert_outside_untouched(outside.path(), &seeded);
    }

    #[test]
    fn observe_refuses_when_traces_dir_is_a_symlink() {
        let repo = fixture_repo();
        let (outside, seeded) = outside_dir();

        // A real `.aoa` with a planted `traces` link: lstat of `.aoa/traces`
        // catches this one, whereas lstat of `.aoa` alone would not.
        std::fs::create_dir(repo.path().join(".aoa")).expect("create real .aoa");
        let planted = repo.path().join(".aoa").join("traces");
        symlink(outside.path(), &planted).expect("plant traces symlink");

        let err = observe(repo.path()).expect_err("observe must refuse a symlinked traces dir");
        assert_unsafe_install_path(err, &planted);
        assert_outside_untouched(outside.path(), &seeded);
    }

    #[test]
    fn observe_refuses_when_gitignore_is_a_symlink() {
        let repo = fixture_repo();
        let (outside, seeded) = outside_dir();

        std::fs::create_dir(repo.path().join(".aoa")).expect("create real .aoa");
        let planted = repo.path().join(".aoa").join(".gitignore");
        symlink(&seeded, &planted).expect("plant .gitignore symlink");

        let err = observe(repo.path()).expect_err("observe must refuse a symlinked .gitignore");
        assert_unsafe_install_path(err, &planted);
        assert_outside_untouched(outside.path(), &seeded);
    }

    #[test]
    fn observe_refuses_to_truncate_a_hardlinked_gitignore() {
        let repo = fixture_repo();
        std::fs::create_dir(repo.path().join(".aoa")).expect("create .aoa");
        let victim = repo.path().join("src.rs");
        let planted = repo.path().join(".aoa/.gitignore");
        std::fs::hard_link(&victim, &planted).expect("plant hardlink");

        observe(repo.path()).expect_err("observe must refuse a hardlinked .gitignore");
        assert_eq!(
            std::fs::read_to_string(&victim).expect("read victim"),
            "fn main() {}\n",
            "observe truncated the tracked hardlink peer"
        );
    }

    #[test]
    fn observe_refuses_a_fifo_gitignore_without_hanging() {
        use std::sync::mpsc;
        use std::time::Duration;

        let repo = fixture_repo();
        std::fs::create_dir(repo.path().join(".aoa")).expect("create .aoa");
        let planted = repo.path().join(".aoa/.gitignore");
        let made = std::process::Command::new("mkfifo")
            .arg(&planted)
            .status()
            .expect("mkfifo is available");
        assert!(made.success(), "mkfifo failed");

        let (tx, rx) = mpsc::channel();
        let repo_path = repo.path().to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(observe(&repo_path).is_err());
        });
        let refused = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("observe must return rather than block on the FIFO");
        assert!(refused, "a FIFO at .aoa/.gitignore must be refused");
    }

    // The guard runs at WRITE time too, not only at install time: `observe()`
    // runs once but writes happen in later processes, so a link planted after
    // install would otherwise defeat the whole fix.
    #[test]
    fn write_trace_refuses_when_the_traces_dir_became_a_symlink() {
        let repo = fixture_repo();
        let (outside, seeded) = outside_dir();

        let outcome = observe(repo.path()).expect("observe succeeds on a clean repo");

        // Swap the installed traces dir for a link, exactly as a later attacker
        // with write access to `.aoa` would.
        std::fs::remove_dir(&outcome.traces_dir).expect("remove installed traces dir");
        symlink(outside.path(), &outcome.traces_dir).expect("plant traces symlink");

        let err = write_trace(&outcome, "run-1.json", &valid_trace())
            .expect_err("write_trace must refuse a symlinked traces dir");
        assert_unsafe_install_path(err, &outcome.traces_dir);
        assert_outside_untouched(outside.path(), &seeded);
    }
}

// The install-path guard must not break the ordinary case: a clean repo
// installs, and a re-install over the real `.aoa/traces` it just created still
// succeeds. Not confined to the unix module above — this one must hold on every
// platform the guard runs on.
#[test]
fn observe_is_idempotent_on_a_clean_repo() {
    let repo = fixture_repo();

    let first = observe(repo.path()).expect("first install succeeds");
    let second = observe(repo.path()).expect("re-install over a real .aoa/traces succeeds");

    assert_eq!(first.traces_dir, second.traces_dir);
    assert!(
        second.traces_dir.is_dir(),
        "traces dir must exist after install"
    );
    assert_eq!(
        std::fs::read_to_string(&second.gitignore).expect("read .gitignore"),
        "*\n"
    );
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
        subtree: None,
    };
    let tier2_item = aoa_audit::PunchItem {
        title: "tier2 gap".into(),
        kind: aoa_audit::FindingKind::MissingPlane,
        tier: Tier::Tier2,
        measured_cost: aoa_audit::MeasuredCost::new(1, "missing plane"),
        plane: Some(aoa_audit::EnforcementPlane::PreCommit),
        subtree: None,
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

// aoa-d6t.31: a workspace repo's punch-list scopes path-carrying findings to
// their member subtree on the wire, and unattributed items omit the key.
#[test]
fn audit_scopes_findings_to_workspace_subtrees() {
    let repo = tempfile::tempdir().expect("temp repo");
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\", \"crates/bar\"]\n",
    )
    .expect("write workspace manifest");
    std::fs::write(repo.path().join("README.md"), "# root\n").expect("write root README");
    for member in ["foo", "bar"] {
        let root = repo.path().join("crates").join(member);
        std::fs::create_dir_all(&root).expect("create member");
        std::fs::write(root.join("Cargo.toml"), "[package]\n").expect("write member manifest");
    }
    // Only crates/foo lacks a README: the navigability finding is scoped to it.
    std::fs::write(repo.path().join("crates/bar/README.md"), "# bar\n").expect("write bar README");

    let report = audit(repo.path(), &AuditConfig::default()).expect("audit succeeds");
    let json = serde_json::to_value(&report).expect("serialize report");

    let items = json["items"].as_array().expect("items array");
    let anchor = items
        .iter()
        .find(|item| item["kind"] == "navigability_anchor")
        .expect("navigability item");
    assert_eq!(
        anchor["subtree"], "crates/foo",
        "the single-member finding must carry its subtree on the wire"
    );

    // A finding with no path (a missing enforcement plane) never carries the key.
    let plane = items
        .iter()
        .find(|item| item["kind"] == "missing_plane")
        .expect("missing-plane item");
    assert!(
        plane.as_object().expect("object").get("subtree").is_none(),
        "an unattributed item must omit the subtree key entirely"
    );

    // A clean discovery carries no warning key on the wire.
    assert!(
        json.as_object()
            .expect("report object")
            .get("subtree_discovery_warning")
            .is_none(),
        "a clean discovery must omit the warning key entirely"
    );
}

// A malformed workspace manifest must not cost the operator the whole punch
// list: the audit degrades to repo-wide findings (no subtree labels) and
// surfaces the discovery failure on the report instead of aborting. This is
// the `discover_partition` doc contract ("callers should surface that and
// fall back to repo-wide reporting, never guess") and matches the eval-run
// CLI's warn-and-degrade treatment.
#[test]
fn audit_degrades_to_repo_wide_when_workspace_manifest_is_malformed() {
    let repo = tempfile::tempdir().expect("temp repo");
    // A syntactically invalid package.json: not even a workspace, but the
    // parse failure must still degrade, not abort.
    std::fs::write(repo.path().join("package.json"), "{ \"name\": \"x\", }")
        .expect("write malformed manifest");
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").expect("write source");

    let report = audit(repo.path(), &AuditConfig::default()).expect("audit must not abort");

    assert!(!report.items.is_empty(), "the punch list must survive");
    assert!(
        report.items.iter().all(|item| item.subtree.is_none()),
        "degraded discovery must leave every finding repo-wide"
    );
    let warning = report
        .subtree_discovery_warning
        .as_deref()
        .expect("the discovery failure must be surfaced, not swallowed");
    assert!(
        warning.contains("package.json"),
        "warning must name the offending manifest: {warning}"
    );

    // The warning rides the wire and the human render.
    let json = serde_json::to_value(&report).expect("serialize report");
    assert!(json["subtree_discovery_warning"]
        .as_str()
        .expect("warning on the wire")
        .contains("package.json"));
    assert!(
        report.render_human().contains(warning),
        "human render must surface the degradation"
    );
}

// The same degrade contract for a symlinked manifest (rejected, never
// followed): the punch list survives with the failure surfaced.
#[cfg(unix)]
#[test]
fn audit_degrades_to_repo_wide_when_workspace_manifest_is_a_symlink() {
    let repo = tempfile::tempdir().expect("temp repo");
    std::fs::write(repo.path().join("real.toml"), "[workspace]\nmembers = []\n")
        .expect("write target");
    std::os::unix::fs::symlink(
        repo.path().join("real.toml"),
        repo.path().join("Cargo.toml"),
    )
    .expect("symlink manifest");
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").expect("write source");

    let report = audit(repo.path(), &AuditConfig::default()).expect("audit must not abort");
    assert!(!report.items.is_empty(), "the punch list must survive");
    let warning = report
        .subtree_discovery_warning
        .as_deref()
        .expect("symlink rejection must be surfaced");
    assert!(
        warning.contains("Cargo.toml"),
        "warning must name the offending manifest: {warning}"
    );
}

/// Accumulate `n` observe-captured live sessions that each carry a landed
/// edit — the held-out ground truth that makes a session count as one
/// behavioral observation.
fn seed_edit_sessions(repo: &Path, n: usize) {
    let traces = repo.join(".aoa").join("traces");
    std::fs::create_dir_all(&traces).expect("create traces dir");
    // The attempt records intent before the tool runs; the committed span is
    // what attests the edit landed. Only the latter makes the session count as
    // a held-out observation.
    let spans = concat!(
        r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#,
        "\n",
        r#"{"type":"write.attempt","source":"native","seq":1,"attributes":{"path":"src/lib.rs"}}"#,
        "\n",
        r#"{"type":"write.committed","source":"native","seq":2,"attributes":{"path":"src/lib.rs"}}"#,
        "\n",
    );
    for i in 0..n {
        std::fs::write(traces.join(format!("live-s{i}.jsonl")), spans).expect("write live log");
    }
}

// aoa-d6t.23 criterion: a repo with no observe-captured held-out signal
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
    // Metrics and reason both match the canonical family note.
    assert_eq!(*note, aoa_construct::InsufficientDataNote::behavioral());

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
    assert!(
        human.contains(aoa_construct::INSUFFICIENT_DATA_REASON),
        "{human}"
    );
    assert_eq!(
        report.determination(),
        aoa_construct::determination_with_signal(&report.behavioral_signal),
        "the report owns derivation of its signal-conditioned determination"
    );
}

// aoa-d6t.23 criterion: once enough observe-captured sessions accumulate under
// .aoa/traces, the behavioral metrics light up (the mutation-surface item is
// emitted again and the insufficient-data note disappears).
#[test]
fn accumulated_trace_corpus_lights_the_behavioral_metrics_up() {
    let repo = fixture_repo();
    seed_edit_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);

    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");
    assert_eq!(
        report.behavioral_signal.observations,
        MIN_HELD_OUT_OBSERVATIONS
    );
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

// aoa-d6t.23 review finding: crossing the window must not re-enable a score
// computed from nothing. With a sufficient corpus but an empty symbol graph
// (the AuditConfig::default() shape) there is no measurement, so the
// mutation-surface item stays out — "0 writable files reachable" would be a
// fabricated claim, not a measured one.
#[test]
fn sufficient_signal_with_an_empty_graph_emits_no_fabricated_surface_score() {
    let repo = fixture_repo();
    seed_edit_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);

    let report = audit(repo.path(), &AuditConfig::default()).expect("audit succeeds");
    assert!(report.behavioral_signal.is_sufficient());
    assert!(
        !report
            .items
            .iter()
            .any(|i| i.kind == aoa_audit::FindingKind::MutationSurface),
        "an empty graph measures nothing; no score may be emitted"
    );
    assert!(
        !report.render_human().contains("0 writable files reachable"),
        "the fabricated zero must never render"
    );
}

// aoa-d6t.23 review finding: the window must not be satisfiable by sessions
// that carry no held-out signal — a full window's worth of edit-free live
// logs is zero observations, and the behavioral item stays withheld.
#[test]
fn edit_free_sessions_do_not_satisfy_the_behavioral_window() {
    let repo = fixture_repo();
    let traces = repo.path().join(".aoa").join("traces");
    std::fs::create_dir_all(&traces).expect("create traces dir");
    let span = r#"{"type":"test.run","source":"native","seq":0,"attributes":{}}"#;
    for i in 0..MIN_HELD_OUT_OBSERVATIONS {
        std::fs::write(traces.join(format!("live-s{i}.jsonl")), format!("{span}\n"))
            .expect("write live log");
    }

    let report = audit(repo.path(), &audit_config()).expect("audit succeeds");
    assert_eq!(
        report.behavioral_signal.observations, 0,
        "edit-free sessions supply no held-out signal"
    );
    assert!(report.insufficient_data.is_some());
    assert!(!report
        .items
        .iter()
        .any(|i| i.kind == aoa_audit::FindingKind::MutationSurface));
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

// aoa-d6t.38: the `serde(default)` on `behavioral_signal` only ever covered a
// MISSING struct. A present-but-tampered one used to be trusted verbatim, so a
// hand-edited report could lower the window and hand a greenfield repo
// sufficient behavioral signal. The threshold is now re-derived on ingest.
#[test]
fn tampered_report_json_cannot_forge_sufficient_behavioral_signal() {
    let forged = r#"{"items":[],"behavioral_signal":{"observations":1,"required":1}}"#;
    let report: AuditReport = serde_json::from_str(forged).expect("deserializes");

    assert_eq!(
        report.behavioral_signal.required, MIN_HELD_OUT_OBSERVATIONS,
        "the reader's own calibration floor governs, not the wire's"
    );
    assert!(
        !report.behavioral_signal.is_sufficient(),
        "one observation cannot satisfy the window however the report is edited"
    );
}

#[test]
fn deserialized_report_rederives_insufficient_data_from_signal() {
    let suppressed =
        r#"{"items":[],"behavioral_signal":{"observations":0},"insufficient_data":null}"#;
    let report: AuditReport = serde_json::from_str(suppressed).expect("deserializes");

    assert!(
        report.insufficient_data.is_some(),
        "wire data cannot suppress the note derived from an insufficient signal"
    );
    assert!(
        report.render_human().contains("[InsufficientData]"),
        "the human register must disclose withheld behavioral metrics"
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
