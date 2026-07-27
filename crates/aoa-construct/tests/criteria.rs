use aoa_construct::{
    classify_metric, CorrelationReport, ExternalOutcome, GatingThresholds, MetricMode,
    MetricOrientation, OutcomeCorrelation, GATING_CANDIDATES, STRUCTURE_MEASURE_SPECS,
};

// Criterion 6: a metric is advisory unless a correlation report supplies a
// CONFIRMING correlation — right sign for the metric's orientation, magnitude at
// or above the floor, sample at or above the floor, and significant. Each axis
// failing independently keeps the metric advisory; only all four together gate.
#[test]
fn criterion_6_construct_validity() {
    let t = GatingThresholds::default(); // min |rho| 0.3, min n 5, alpha 0.05

    // A correlation for `edit_locality` (HigherIsBetter) against an outcome,
    // with explicit coefficient/n/p so each gate is exercised in isolation.
    let corr = |outcome, coefficient, n, p_value| CorrelationReport {
        metric: "edit_locality".into(),
        orientation: MetricOrientation::HigherIsBetter,
        correlations: vec![OutcomeCorrelation {
            outcome,
            coefficient,
            n,
            p_value,
        }],
    };

    // No report at all -> advisory.
    assert_eq!(classify_metric(None, &t), MetricMode::Advisory);

    // Empty report (no external outcomes available) -> advisory.
    let empty = CorrelationReport {
        metric: "edit_locality".into(),
        orientation: MetricOrientation::HigherIsBetter,
        correlations: vec![],
    };
    assert_eq!(classify_metric(Some(&empty), &t), MetricMode::Advisory);

    // Wrong direction: a HigherIsBetter metric POSITIVELY correlated with the
    // revert rate (more locality -> more reverts) refutes, not confirms.
    let wrong_dir = corr(ExternalOutcome::RevertRate, 0.8, 8, 0.01);
    assert_eq!(classify_metric(Some(&wrong_dir), &t), MetricMode::Advisory);

    // Below magnitude: correct sign but |rho| under the floor.
    let weak = corr(ExternalOutcome::ReviewAcceptance, 0.2, 8, 0.01);
    assert_eq!(classify_metric(Some(&weak), &t), MetricMode::Advisory);

    // Below sample size: strong and significant but too few observations.
    let tiny = corr(ExternalOutcome::ReviewAcceptance, 0.9, 4, 0.01);
    assert_eq!(classify_metric(Some(&tiny), &t), MetricMode::Advisory);

    // Not significant: strong, right sign, big enough sample, but p > alpha.
    let noisy = corr(ExternalOutcome::ReviewAcceptance, 0.9, 8, 0.20);
    assert_eq!(classify_metric(Some(&noisy), &t), MetricMode::Advisory);

    // Confirming for a HigherIsBetter metric: positive vs review-acceptance
    // (more locality -> more accepted), and negative vs revert/incident (more
    // locality -> fewer harms). All clear the thresholds -> gating.
    let confirming = [
        (ExternalOutcome::ReviewAcceptance, 0.8_f64),
        (ExternalOutcome::RevertRate, -0.7),
        (ExternalOutcome::IncidentCount, -0.7),
    ];
    for (outcome, coefficient) in confirming {
        let report = corr(outcome, coefficient, 8, 0.01);
        assert_eq!(
            classify_metric(Some(&report), &t),
            MetricMode::Gating,
            "outcome {outcome:?} should confirm a HigherIsBetter metric"
        );
    }
}

// Criterion 6 (orientation): a LowerIsBetter metric flips the confirming sign.
// `mutation_surface` (smaller blast radius is better) POSITIVELY correlated with
// the revert rate confirms (more surface -> more reverts); a negative
// correlation would refute.
#[test]
fn criterion_6_lower_is_better_orientation() {
    let t = GatingThresholds::default();
    let surface = |coefficient| CorrelationReport {
        metric: "mutation_surface".into(),
        orientation: MetricOrientation::LowerIsBetter,
        correlations: vec![OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            coefficient,
            n: 8,
            p_value: 0.01,
        }],
    };
    assert_eq!(classify_metric(Some(&surface(0.7)), &t), MetricMode::Gating);
    assert_eq!(
        classify_metric(Some(&surface(-0.7)), &t),
        MetricMode::Advisory
    );

    // The sixth combination: a LowerIsBetter harm metric vs review-acceptance
    // (higher is better) confirms with a NEGATIVE coefficient (less harm -> more
    // accepted); a positive coefficient refutes.
    let surface_vs_accept = |coefficient| CorrelationReport {
        metric: "mutation_surface".into(),
        orientation: MetricOrientation::LowerIsBetter,
        correlations: vec![OutcomeCorrelation {
            outcome: ExternalOutcome::ReviewAcceptance,
            coefficient,
            n: 8,
            p_value: 0.01,
        }],
    };
    assert_eq!(
        classify_metric(Some(&surface_vs_accept(-0.7)), &t),
        MetricMode::Gating
    );
    assert_eq!(
        classify_metric(Some(&surface_vs_accept(0.7)), &t),
        MetricMode::Advisory
    );
}

// R9c structure-measure wiring (aoa-mnz.3): the code-structure measures are
// registered as Advisory gating candidates, born advisory exactly like the
// process metrics, and gain no special treatment. Registration itself is
// type-level now (a spec is keyed on MetricName, and every MetricName is a
// GATING_CANDIDATES entry by construction) — what stays asserted is policy.
#[test]
fn structure_measures_are_registered_advisory_candidates() {
    for (metric, _) in STRUCTURE_MEASURE_SPECS {
        // A count of a harm (absences / outliers / unused imports) is
        // LowerIsBetter.
        assert_eq!(
            metric.orientation(),
            MetricOrientation::LowerIsBetter,
            "{metric} must be a LowerIsBetter gating candidate"
        );
    }

    // Born advisory: absent any external-outcome correlation, no report
    // classifies as gating. This holds for every candidate regardless of
    // metric, so it is asserted once rather than per iteration.
    assert_eq!(
        classify_metric(None, &GatingThresholds::default()),
        MetricMode::Advisory
    );
}

// The pre-registered spec is a single source of truth on direction: every spec
// metric carries a non-empty mechanical definition — the spec AOA verifies,
// not defines. (That each spec names a real candidate is enforced by the
// MetricName key.)
#[test]
fn structure_measure_spec_is_documented() {
    for (metric, definition) in STRUCTURE_MEASURE_SPECS {
        assert!(
            !definition.trim().is_empty(),
            "spec metric {metric} has no pre-registered definition"
        );
        assert!(
            !definition.contains("aoa-audit")
                && !definition.contains("_item")
                && !definition.contains("_sites"),
            "spec metric {metric} leaks a provider implementation detail: {definition}"
        );
    }
}

// AC3: a structure measure CAN promote to Gating when a confirming
// external-outcome correlation exists — the same path as the process metrics.
// For a LowerIsBetter metric against a lower-is-better outcome (revert rate),
// metric-good and outcome-good point the same way, so a POSITIVE coefficient
// confirms (fewer unused imports track fewer reverts).
#[test]
fn structure_measure_promotes_to_gating_on_confirming_correlation() {
    let t = GatingThresholds::default();
    let report = CorrelationReport {
        metric: "unused_import_proxy".into(),
        orientation: MetricOrientation::LowerIsBetter,
        correlations: vec![OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            coefficient: 0.8,
            n: 12,
            p_value: 0.001,
        }],
    };
    assert_eq!(classify_metric(Some(&report), &t), MetricMode::Gating);

    // The same evidence with the WRONG sign (negative coefficient) does not
    // confirm a LowerIsBetter-vs-revert hypothesis — it stays advisory.
    let wrong_sign = CorrelationReport {
        correlations: vec![OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            coefficient: -0.8,
            n: 12,
            p_value: 0.001,
        }],
        ..report
    };
    assert_eq!(classify_metric(Some(&wrong_sign), &t), MetricMode::Advisory);
}

// Criterion 6 (artifact): the current determination is a reproducible artifact
// that names its data source and classifies every gating candidate. With no
// external-outcome corpus available, every candidate is advisory — the
// executable form of "no metric gates a feature without real correlation". The
// committed fixture is byte-for-byte reproduced by the pipeline.
#[test]
fn criterion_6_artifact_reproduces_and_all_advisory() {
    let artifact = aoa_construct::current_determination();

    // Every gating candidate appears, and none gates absent external data.
    assert_eq!(artifact.metrics.len(), GATING_CANDIDATES.len());
    for (m, expected) in artifact.metrics.iter().zip(GATING_CANDIDATES) {
        assert_eq!(m.metric, expected.0.as_str());
        assert_eq!(m.orientation, expected.1);
        assert!(m.correlations.is_empty());
        assert_eq!(
            m.mode,
            MetricMode::Advisory,
            "{} must stay advisory",
            m.metric
        );
    }
    assert!(!artifact.data_source.is_empty());

    // The committed artifact fixture deserializes to exactly this determination,
    // and the value round-trips through serde unchanged.
    let fixture = include_str!("fixtures/construct_validity_report.json");
    let from_disk: aoa_construct::ConstructValidityReport =
        serde_json::from_str(fixture).expect("fixture parses");
    assert_eq!(
        from_disk, artifact,
        "fixture must match the pipeline output"
    );

    let roundtrip: aoa_construct::ConstructValidityReport =
        serde_json::from_str(&serde_json::to_string(&artifact).unwrap()).unwrap();
    assert_eq!(roundtrip, artifact);
}
