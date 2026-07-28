use super::behavioral_signal::{seed_live_sessions, INSUFFICIENT_REASON};
use super::*;

// --- aoa report (aoa-d6t.19, leg 1) ------------------------------------------
// One end-to-end operator readiness view composing the audit punch-list, the
// R9c Advisory/Gating determination, the migration registry, the recommend
// join, and (when present) the R0 falsification verdict.

#[test]
fn report_composes_all_pillars_and_reports_absent_falsification() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["report", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("punch-list"))
        .stdout(predicate::str::contains("construct validity"))
        .stdout(predicate::str::contains("navigability-anchor"))
        .stdout(predicate::str::contains("AOA recommendations"))
        // Absent input is reported as absent, never fabricated.
        .stdout(predicate::str::contains("falsification.json: absent"));
}

#[test]
fn report_json_composes_pillars_and_pillar_is_not_live_without_falsification() {
    let repo = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed["audit"]["items"].is_array());
    assert!(parsed["construct_validity"]["metrics"].is_array());
    assert!(
        !parsed["migrations"]
            .as_array()
            .expect("migrations")
            .is_empty(),
        "the migration registry is surfaced"
    );
    assert!(parsed["recommendations"]["items"].is_array());
    assert_eq!(parsed["falsification"]["status"], "absent");
    assert_eq!(parsed["migrate_pillar_live"], false);
}

// aoa-d6t.35: `report` composes audit + determination + recommend into ONE
// document, so it must condition the determination on the same behavioral
// signal the audit measured — otherwise the halves contradict each other.
#[test]
fn report_json_conditions_the_determination_on_the_behavioral_signal() {
    let repo = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    // The audit half already reports the shortfall.
    assert_eq!(
        parsed["audit"]["insufficient_data"]["reason"],
        INSUFFICIENT_REASON
    );

    // The determination must agree: all four behavioral metrics tagged
    // insufficient_data, never advisory.
    let metrics = parsed["construct_validity"]["metrics"]
        .as_array()
        .expect("metrics");
    let insufficient = metrics
        .iter()
        .filter(|m| m["mode"] == "insufficient_data")
        .count();
    assert_eq!(insufficient, 4, "the four locality metrics: {metrics:#?}");
    assert!(
        metrics
            .iter()
            .any(|m| m["metric"] == "retrieval_locality" && m["mode"] == "insufficient_data"),
        "retrieval_locality must not be advisory on a greenfield repo: {metrics:#?}"
    );

    // And the recommend join carries the note, as `aoa recommend` does.
    assert_eq!(
        parsed["recommendations"]["insufficient_data"]["reason"],
        INSUFFICIENT_REASON
    );
}

// Raw landed edits are not complete four-metric evidence, regardless of count.
#[test]
fn report_json_keeps_uncontextualized_live_edits_insufficient() {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), 10);
    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    assert!(parsed["audit"].get("insufficient_data").is_some());
    assert!(parsed["recommendations"].get("insufficient_data").is_some());
    let metrics = parsed["construct_validity"]["metrics"]
        .as_array()
        .expect("metrics");
    assert!(
        metrics.iter().any(|m| m["mode"] == "insufficient_data"),
        "uncontextualized edits leave metrics insufficient: {metrics:#?}"
    );
}

// aoa-d6t.41: `report` must audit through the same repo-aware config as `audit`
// and `recommend`. Building an `AuditConfig::default()` here hands the audit an
// empty symbol graph, which silently withholds the measured mutation-surface
// item — the operator-facing readiness view under-reporting the very findings
// the other two surfaces report for the same repo.
#[test]
fn report_and_audit_agree_on_findings_for_an_indexable_repo() {
    let repo = seeded_indexable_repo();

    // Compare the whole AuditReport, not just each item's `kind`. A kind-only
    // comparison passes while `measured_cost` or `subtree` diverge, and
    // `subtree` is observable through this leg alone: `FindingRecommendation`
    // drops it, so the recommend-leg guard below cannot see it.
    let from_audit = aoa_json(repo.path(), &["audit", "--json", "--repo"]);
    // `audit --json` IS the AuditReport; `report --json` nests it under `audit`.
    let mut report_view = aoa_json(repo.path(), &["report", "--json", "--repo"]);
    let from_report = report_view["audit"].take();

    assert_eq!(
        from_audit["live_observations"]
            .as_array()
            .expect("live observations")
            .len(),
        10,
        "precondition: the fixture carries raw live sessions"
    );
    assert_eq!(
        from_report, from_audit,
        "`report` must not diverge from the audit `audit` reports for the same repo"
    );
}

/// Run `aoa` with `args` against `repo` and parse stdout as JSON.
fn aoa_json(repo: &Path, args: &[&str]) -> Value {
    let output = aoa().args(args).arg(repo).output().expect("run");
    assert!(output.status.success(), "{args:?} failed: {output:?}");
    serde_json::from_slice(&output.stdout).expect("valid json")
}

/// A repo with BOTH a sufficient held-out corpus and source the indexer can see
/// into. Both halves are load-bearing for the agreement guards: without the
/// corpus the commands take the InsufficientData path, and without a `.py` file
/// the symbol graph is empty for every command, so neither alone would catch a
/// config or conditioning divergence.
pub(super) fn seeded_indexable_repo() -> TempDir {
    let repo = TempDir::new().expect("tempdir");
    seed_live_sessions(repo.path(), MIN_HELD_OUT_OBSERVATIONS);
    std::fs::write(
        repo.path().join("app.py"),
        "def handle(x):\n    return store(x)\n\ndef store(x):\n    return x\n",
    )
    .expect("write indexable source");
    repo
}

// aoa-ghwi: the same guard for the RECOMMEND leg of `report`. aoa-d6t.41 locked
// report's audit half to `aoa audit` and left this half uncovered, which is
// where a third divergence lands: `report::run` was a statement-for-statement
// hand-copy of `recommend::run`, so any step `recommend` gains and `report`
// misses ships a readiness view that under-reports while exiting 0.
//
// Two fixtures, because neither alone covers both historical divergence
// classes. On the seeded+indexable repo `determination_with_signal` returns the
// ordinary determination, so a fork that drops the signal conditioning
// (aoa-d6t.35) is invisible; on the greenfield repo the symbol graph is empty,
// so a fork that rebuilds the audit config by hand (aoa-d6t.41) is invisible.
#[test]
fn report_and_recommend_agree_on_recommendations() {
    let greenfield = TempDir::new().expect("tempdir");
    let indexable = seeded_indexable_repo();

    // Assert the two surfaces agree for `repo`, and hand back the report view so
    // the preconditions below read it instead of re-running the command.
    let agreeing_report = |repo: &Path| -> Value {
        let from_recommend = aoa_json(repo, &["recommend", "--json", "--repo"]);
        let from_report = aoa_json(repo, &["report", "--json", "--repo"]);
        assert_eq!(
            from_report["recommendations"],
            from_recommend,
            "`report` must join recommendations exactly as `recommend` does, for {}",
            repo.display()
        );
        from_report
    };

    let greenfield_view = agreeing_report(greenfield.path());
    let indexable_view = agreeing_report(indexable.path());

    // Precondition: the two fixtures must actually exercise the two different
    // legs, or the pair above is one assertion run twice. The indexable fixture
    // has live edit candidates, while the greenfield fixture has none. Both
    // remain insufficient because ambient sessions lack same-task context.
    let live_observation_count = |view: &Value| -> usize {
        view["audit"]
            .get("live_observations")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    let has_insufficient_data = |view: &Value| -> bool {
        view["construct_validity"]["metrics"]
            .as_array()
            .expect("metrics")
            .iter()
            .any(|m| m["mode"] == "insufficient_data")
    };

    for (name, view, observations) in [
        ("greenfield", &greenfield_view, 0),
        ("indexable", &indexable_view, 10),
    ] {
        assert_eq!(
            live_observation_count(view),
            observations,
            "precondition: only the indexable fixture carries live candidates ({name})"
        );
        assert!(
            has_insufficient_data(view),
            "ambient live candidates remain insufficient without task context ({name})"
        );
    }
}

// aoa-ghwi: the third leg of the same agreement — all three commands must apply
// the same indexing policy. A source file the heuristic scanner cannot decode
// is isolated like an oversized generated file; valid siblings still supply
// the graph instead of one irrelevant file aborting the repository-wide audit.
#[test]
fn audit_recommend_and_report_isolate_a_non_utf8_source_file() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("generated.py"),
        b"def f():\n    x = \"\xff\xfe\"\n",
    )
    .expect("write non-utf8 source");
    std::fs::write(repo.path().join("app.py"), "def valid():\n    pass\n")
        .expect("write valid source");

    for command in ["audit", "recommend", "report"] {
        aoa()
            .args([command, "--repo"])
            .arg(repo.path())
            .assert()
            .success();
    }
}

#[test]
fn report_proceed_verdict_marks_the_migrate_pillar_live() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("falsification.json"),
        r#"{"verdict":"proceed","notes":[]}"#,
    )
    .unwrap();

    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["falsification"]["status"], "present");
    assert_eq!(parsed["falsification"]["verdict"], "proceed");
    assert_eq!(parsed["migrate_pillar_live"], true);
}

#[test]
fn report_pivot_verdict_keeps_the_migrate_pillar_not_live() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("falsification.json"),
        r#"{"verdict":"pivot","notes":[]}"#,
    )
    .unwrap();

    aoa()
        .args(["report", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("pivot"))
        .stdout(predicate::str::contains("not live"));
}

#[test]
fn report_precondition_unmet_verdict_is_surfaced_and_not_live() {
    // An inconclusive written by an unmet precondition (e.g. too_few_repos)
    // carries its discriminator; the report surfaces it and the pillar stays
    // not live.
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("falsification.json"),
        r#"{"verdict":"inconclusive","precondition_unmet":"too_few_repos","notes":[]}"#,
    )
    .unwrap();

    let output = aoa()
        .args(["report", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(
        parsed["falsification"]["precondition_unmet"],
        "too_few_repos"
    );
    assert_eq!(parsed["migrate_pillar_live"], false);
}

#[test]
fn report_fails_loud_on_malformed_falsification_json() {
    // A present-but-unparsable falsification.json is a hard error, never
    // silently treated as absent (that would fabricate "gate never ran").
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(repo.path().join("falsification.json"), "not json").unwrap();

    aoa()
        .args(["report", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("falsification.json"));
}
