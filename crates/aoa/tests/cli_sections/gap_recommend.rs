use super::*;

// aoa-mnz.7: the `aoa gap` subcommand is a live, non-test consumer of
// `current_determination()`. It surfaces the R9c Gating-vs-Advisory
// determination to the operator. With no external-outcome corpus available,
// every gating candidate is Advisory — the surface must say so, naming each
// candidate, rather than silently gating.
#[test]
fn gap_human_surfaces_advisory_determination() {
    aoa()
        .args(["gap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("construct validity"))
        .stdout(predicate::str::contains("Advisory"))
        // a known pre-registered candidate is named
        .stdout(predicate::str::contains("reward_hacking_gap"));
}

#[test]
fn gap_json_carries_every_candidate_as_advisory() {
    let output = aoa().args(["gap", "--json"]).output().expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let metrics = parsed["metrics"].as_array().expect("metrics array");
    assert!(!metrics.is_empty(), "every gating candidate is classified");
    for m in metrics {
        assert_eq!(
            m["mode"], "advisory",
            "no candidate gates without an external-outcome corpus"
        );
    }
    assert!(
        parsed["data_source"].as_str().unwrap().contains("external"),
        "the surface names the data source it consulted"
    );
}

// aoa-d6t.15: the `aoa recommend` subcommand is the connective tissue — it joins
// audit findings + the construct-validity determination + migration availability
// into per-finding recommendations. With no external-outcome corpus, every metric
// is Advisory, so every finding is advisory-only; the surface must say so and
// name the fix availability, never asserting a fix is worth applying.
#[test]
fn recommend_human_surfaces_advisory_findings() {
    // A bare repo is missing Tier-1 planes and has no README -> several findings.
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["recommend", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("AOA recommendations"))
        .stdout(predicate::str::contains("advisory-only"))
        // The footer ties the empty actionable set back to the gap determination.
        .stdout(predicate::str::contains("aoa gap"));
}

#[test]
fn recommend_json_joins_findings_with_metric_and_fix() {
    // A manifest-bearing root without a README yields a navigability finding that
    // HAS a fix (navigability-anchor) but whose metric is Advisory -> the join
    // tags it advisory-only with the metric-advisory reason, fix surfaced.
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();

    let output = aoa()
        .args(["recommend", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success(), "recommend is advisory, exits zero");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    // Counts are present and, with no corpus, nothing is actionable-now.
    assert_eq!(parsed["actionable_now"], 0);
    assert!(parsed["advisory_only"].as_u64().unwrap() >= 1);

    let items = parsed["items"].as_array().expect("items array");
    let nav = items
        .iter()
        .find(|i| i["kind"] == "navigability_anchor")
        .expect("navigability finding present");
    assert_eq!(nav["actionability"], "advisory_only");
    assert_eq!(nav["advisory_reason"], "metric_advisory");
    assert_eq!(nav["metric"], "navigability_anchor_absence");
    assert_eq!(nav["metric_mode"], "advisory");
    assert_eq!(nav["fix_id"], "navigability-anchor");
    assert!(
        nav["fix_eligibility"]
            .as_str()
            .unwrap()
            .contains("code-layer"),
        "the fix's eligibility precondition is surfaced"
    );

    // A missing-plane finding has no gating-candidate metric and no fix: the join
    // distinguishes "no metric" from "metric advisory".
    let plane = items
        .iter()
        .find(|i| i["kind"] == "missing_plane")
        .expect("missing-plane finding present");
    assert!(plane["metric"].is_null(), "plane gap has no metric");
    assert!(plane["fix_id"].is_null(), "no migration for a plane gap");
    assert_eq!(plane["advisory_reason"], "no_fix_available");
}
