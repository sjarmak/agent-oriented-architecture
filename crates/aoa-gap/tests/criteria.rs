use aoa_gap::{
    classify_metric, compare, compute_gap, spearman, CanaryItem, CorrelationReport,
    ExternalOutcome, GapError, GapOutcome, GatingThresholds, HeldOutProvenance, Label, MetricMode,
    MetricOrientation, OutcomeCorrelation, RunResult, TaskOutcome, GATING_CANDIDATES,
};

fn task(visible: bool, held_out: bool) -> TaskOutcome {
    TaskOutcome {
        visible_success: visible,
        held_out_success: held_out,
    }
}

/// Build a run with a given provenance, no canaries.
fn run(tasks: Vec<TaskOutcome>, provenance: HeldOutProvenance) -> RunResult {
    RunResult {
        tasks,
        held_out_provenance: provenance,
        canaries: Vec::new(),
    }
}

// Criterion 1: compute a visible-vs-held-out gap and a gap delta between runs.
#[test]
fn criterion_1_gap_and_delta() {
    // baseline: visible 4/4 = 1.0, held-out 2/4 = 0.5, gap 0.5
    let baseline = run(
        vec![
            task(true, true),
            task(true, true),
            task(true, false),
            task(true, false),
        ],
        HeldOutProvenance::NativeComposed,
    );
    let outcome = compute_gap(&baseline).expect("native suite computes a gap");
    match outcome {
        GapOutcome::Available {
            visible_rate,
            held_out_rate,
            gap,
        } => {
            assert_eq!(visible_rate, 1.0);
            assert_eq!(held_out_rate, 0.5);
            assert_eq!(gap, 0.5);
        }
        GapOutcome::Unavailable => panic!("expected an available gap"),
    }

    // migrated: visible 4/4 = 1.0, held-out 3/4 = 0.75, gap 0.25
    let migrated = run(
        vec![
            task(true, true),
            task(true, true),
            task(true, true),
            task(true, false),
        ],
        HeldOutProvenance::NativeComposed,
    );
    let cmp = compare(&baseline, &migrated).expect("comparable runs");
    assert_eq!(cmp.held_out_delta, 0.25);
    assert_eq!(cmp.gap_delta, -0.25);
}

// Criterion 2: `good` ONLY when held-out improves AND gap holds-or-reduces;
// a visible-pass + locality-only improvement is NOT good. Table-driven.
#[test]
fn criterion_2_label_table() {
    struct Case {
        name: &'static str,
        baseline: Vec<TaskOutcome>,
        migrated: Vec<TaskOutcome>,
        expect: Label,
    }

    let cases = vec![
        Case {
            name: "held-out improves and gap shrinks -> good",
            baseline: vec![task(true, false), task(true, false)],
            migrated: vec![task(true, true), task(true, false)],
            expect: Label::Good,
        },
        Case {
            // Visible already maxed in both; only held-out moves, gap shrinks.
            name: "held-out improves, gap holds-or-reduces -> good",
            baseline: vec![task(true, true), task(true, false), task(true, false)],
            migrated: vec![task(true, true), task(true, true), task(true, false)],
            expect: Label::Good,
        },
        Case {
            // Visible-only / locality-only improvement: held-out flat -> not good.
            name: "visible improves but held-out flat -> not good",
            baseline: vec![task(false, false), task(true, false)],
            migrated: vec![task(true, false), task(true, false)],
            expect: Label::NotGood,
        },
        Case {
            // Held-out improves but the gap WIDENS (visible jumps more): not good.
            name: "held-out up but gap widens -> not good",
            baseline: vec![task(false, false), task(false, false)],
            migrated: vec![task(true, true), task(true, false)],
            expect: Label::NotGood,
        },
        Case {
            name: "held-out regresses -> not good",
            baseline: vec![task(true, true), task(true, true)],
            migrated: vec![task(true, true), task(true, false)],
            expect: Label::NotGood,
        },
    ];

    for c in cases {
        let baseline = run(c.baseline, HeldOutProvenance::NativeComposed);
        let migrated = run(c.migrated, HeldOutProvenance::NativeComposed);
        let cmp = compare(&baseline, &migrated).expect("comparable runs");
        assert_eq!(cmp.label, c.expect, "case: {}", c.name);
    }
}

// Criterion 3: leakage canary fails the run when held-out rises while visible is
// unchanged.
#[test]
fn criterion_3_leakage_canary_fails() {
    // visible flat at 1.0 across both; held-out rises 0.5 -> 1.0.
    let baseline = RunResult {
        tasks: vec![task(true, true), task(true, false)],
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![CanaryItem {
            id: "canary-known-fail".into(),
            held_out_success: false,
            expected_held_out: false,
        }],
    };
    let migrated = RunResult {
        tasks: vec![task(true, true), task(true, true)],
        held_out_provenance: HeldOutProvenance::NativeComposed,
        // Known held-out item that should still FAIL flips to pass: contamination.
        canaries: vec![CanaryItem {
            id: "canary-known-fail".into(),
            held_out_success: true,
            expected_held_out: false,
        }],
    };

    let err = compare(&baseline, &migrated).expect_err("leakage must fail the run");
    assert_eq!(err, GapError::LeakageDetected);
}

// Criterion 3 (counterpart): an honest held-out gain (visible also moves, or
// canaries hold) is NOT flagged as leakage.
#[test]
fn criterion_3_no_false_positive() {
    let baseline = RunResult {
        tasks: vec![task(false, false), task(true, false)],
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![CanaryItem {
            id: "c".into(),
            held_out_success: false,
            expected_held_out: false,
        }],
    };
    // Visible ALSO moves (0.5 -> 1.0) and canary holds: honest gain.
    let migrated = RunResult {
        tasks: vec![task(true, true), task(true, false)],
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![CanaryItem {
            id: "c".into(),
            held_out_success: false,
            expected_held_out: false,
        }],
    };
    let cmp = compare(&baseline, &migrated).expect("honest gain is not leakage");
    assert_eq!(cmp.label, Label::Good);
}

// A known-fail canary that flips to pass: the contamination signal both runs
// share in the regression tests below.
fn flipped_canary() -> CanaryItem {
    CanaryItem {
        id: "canary-known-fail".into(),
        held_out_success: true,
        expected_held_out: false,
    }
}

fn held_canary() -> CanaryItem {
    CanaryItem {
        id: "canary-known-fail".into(),
        held_out_success: false,
        expected_held_out: false,
    }
}

// Criterion 3 (regression, aoa-d6t.6): a real leak that ALSO nudges the visible
// leg DOWN by one task out of N must still trip. The old exact-f64 visible_flat
// check failed open here (0.75 != 1.0), suppressing detection.
#[test]
fn criterion_3_leakage_trips_despite_visible_nudge_down() {
    // N=4, visible_tol = 1/4 = 0.25. visible 1.0 -> 0.75 is a one-task nudge.
    let baseline = RunResult {
        tasks: vec![
            task(true, true),
            task(true, false),
            task(true, false),
            task(true, false),
        ], // visible 1.0, held-out 0.25
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![held_canary()],
    };
    let migrated = RunResult {
        tasks: vec![
            task(false, true),
            task(true, true),
            task(true, true),
            task(true, false),
        ], // visible 0.75 (one task flipped), held-out 0.75
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![flipped_canary()],
    };
    let err = compare(&baseline, &migrated).expect_err("1/N visible nudge must not hide leakage");
    assert_eq!(err, GapError::LeakageDetected);
}

// Criterion 3 (regression, aoa-d6t.6): symmetric to the above — a one-task
// nudge UP in the visible leg must also still trip.
#[test]
fn criterion_3_leakage_trips_despite_visible_nudge_up() {
    // N=4, visible 0.75 -> 1.0 is a one-task nudge up; held-out 0.25 -> 0.75.
    let baseline = RunResult {
        tasks: vec![
            task(false, false),
            task(true, false),
            task(true, false),
            task(true, true),
        ], // visible 0.75, held-out 0.25
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![held_canary()],
    };
    let migrated = RunResult {
        tasks: vec![
            task(true, true),
            task(true, true),
            task(true, false),
            task(true, true),
        ], // visible 1.0, held-out 0.75
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![flipped_canary()],
    };
    let err =
        compare(&baseline, &migrated).expect_err("1/N visible nudge up must not hide leakage");
    assert_eq!(err, GapError::LeakageDetected);
}

// Criterion 3 (scope boundary, aoa-d6t.6): a broad gain that moves the visible
// leg well beyond one task's granularity is NOT the leak signature, even if a
// canary flips. Leakage is a held-out-specific rise; broad improvement that
// lifts visible too is honest capability and must not be flagged. This pins the
// deliberate decision to keep the corroborating visible-flat guard rather than
// trip on a flipped canary alone (which over-fires on nondeterministic flips).
#[test]
fn criterion_3_broad_visible_gain_not_flagged_as_leakage() {
    // N=4, visible 0.0 -> 1.0 (delta 1.0 >> 0.25 band): outside the flat band.
    let baseline = RunResult {
        tasks: vec![
            task(false, false),
            task(false, false),
            task(false, false),
            task(false, false),
        ], // visible 0.0, held-out 0.0
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![held_canary()],
    };
    let migrated = RunResult {
        tasks: vec![
            task(true, true),
            task(true, true),
            task(true, true),
            task(true, false),
        ], // visible 1.0, held-out 0.75
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![flipped_canary()],
    };
    // Not flagged as leakage; held-out rose but the gap widened, so NotGood.
    let cmp = compare(&baseline, &migrated).expect("broad visible gain is not leakage");
    assert_eq!(cmp.label, Label::NotGood);
}

// Criterion 3 (boundary, aoa-d6t.6): when the two runs have different task
// counts, the flat band is governed by the SMALLER run (1/min(N)). Here
// min(N)=2 gives a 0.5 band, so a 0.5 visible swing still reads as flat and the
// leak trips; had the band used max(N)=4 (tol 0.25) it would fail open. Pins the
// min-vs-max choice the same-task-set assumption otherwise hides.
#[test]
fn criterion_3_band_uses_smaller_task_count() {
    let baseline = RunResult {
        tasks: vec![task(true, false), task(true, false)], // N=2: visible 1.0, held-out 0.0
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![held_canary()],
    };
    let migrated = RunResult {
        tasks: vec![
            task(true, true),
            task(false, true),
            task(false, true),
            task(true, false),
        ], // N=4: visible 0.5, held-out 0.75
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![flipped_canary()],
    };
    // visible_delta = 0.5 - 1.0 = -0.5; tol = 1/min(2,4) = 0.5 -> flat -> trips.
    let err = compare(&baseline, &migrated).expect_err("min(N) band must keep the leak detectable");
    assert_eq!(err, GapError::LeakageDetected);
}

// Criterion 3 (boundary, aoa-d6t.6): a single-task run has band = 1/1 = 1.0, so
// any visible movement is inside the band — held-out cannot be distinguished
// from broad gain with one task, so a flipped canary plus a held-out rise trips.
#[test]
fn criterion_3_single_task_trips_on_canary_flip() {
    let baseline = RunResult {
        tasks: vec![task(true, false)], // visible 1.0, held-out 0.0
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![held_canary()],
    };
    let migrated = RunResult {
        tasks: vec![task(false, true)], // visible 0.0, held-out 1.0 (full swing)
        held_out_provenance: HeldOutProvenance::NativeComposed,
        canaries: vec![flipped_canary()],
    };
    let err = compare(&baseline, &migrated).expect_err("single-task leak must trip");
    assert_eq!(err, GapError::LeakageDetected);
}

// Criterion 4: held-out synthesized from visible specs is rejected with an error.
#[test]
fn criterion_4_synthesis_rejected() {
    let r = run(
        vec![task(true, true), task(true, false)],
        HeldOutProvenance::SynthesizedFromVisible,
    );
    assert_eq!(
        compute_gap(&r).expect_err("synthesis must be rejected"),
        GapError::SynthesizedHeldOut
    );

    // And it propagates through compare, not silently accepted.
    let clean = run(vec![task(true, true)], HeldOutProvenance::NativeComposed);
    assert_eq!(
        compare(&clean, &r).expect_err("synthesis rejected in compare"),
        GapError::SynthesizedHeldOut
    );
}

// Criterion 5: no native composed held-out suite -> gap unavailable, and any
// attempt to label a migration is refused.
#[test]
fn criterion_5_unavailable_refuses_label() {
    let r = run(
        vec![task(true, true), task(true, false)],
        HeldOutProvenance::None,
    );
    assert_eq!(
        compute_gap(&r).expect("no suite yields Unavailable, not an error"),
        GapOutcome::Unavailable
    );

    let migrated = run(vec![task(true, true)], HeldOutProvenance::None);
    assert_eq!(
        compare(&r, &migrated).expect_err("absent gap must refuse to label"),
        GapError::GapUnavailable
    );

    // Even if the other side has a real suite, an absent gap on either side
    // refuses to gate.
    let real = run(
        vec![task(true, true), task(true, false)],
        HeldOutProvenance::NativeComposed,
    );
    assert_eq!(
        compare(&real, &migrated).expect_err("absent migrated gap refuses to label"),
        GapError::GapUnavailable
    );
}

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

// Criterion 6 (end-to-end): a confirming correlation computed by the real
// Spearman pipeline from observations gates the metric — sign + magnitude come
// from data, not a hand-set flag.
#[test]
fn criterion_6_end_to_end_from_observations() {
    let t = GatingThresholds::default();
    // edit_locality (x) vs review-acceptance rate (y): a strong monotone tie
    // over 6 observations. Perfect monotone at n=6 gives p = 2/720 << 0.05.
    let observations = [
        (0.10, 0.20),
        (0.25, 0.35),
        (0.40, 0.50),
        (0.55, 0.65),
        (0.70, 0.85),
        (0.90, 0.95),
    ];
    let c = spearman(&observations).expect("well-defined correlation");
    assert!(c.coefficient > 0.9 && c.p_value <= 0.05);

    let report = CorrelationReport {
        metric: "edit_locality".into(),
        orientation: MetricOrientation::HigherIsBetter,
        correlations: vec![OutcomeCorrelation {
            outcome: ExternalOutcome::ReviewAcceptance,
            coefficient: c.coefficient,
            n: c.n,
            p_value: c.p_value,
        }],
    };
    assert_eq!(classify_metric(Some(&report), &t), MetricMode::Gating);
}

// Criterion 6 (artifact): the current determination is a reproducible artifact
// that names its data source and classifies every gating candidate. With no
// external-outcome corpus available, every candidate is advisory — the
// executable form of "no metric gates a feature without real correlation". The
// committed fixture is byte-for-byte reproduced by the pipeline.
#[test]
fn criterion_6_artifact_reproduces_and_all_advisory() {
    let artifact = aoa_gap::current_determination();

    // Every gating candidate appears, and none gates absent external data.
    assert_eq!(artifact.metrics.len(), GATING_CANDIDATES.len());
    for (m, expected) in artifact.metrics.iter().zip(GATING_CANDIDATES) {
        assert_eq!(m.metric, expected.0);
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
    let from_disk: aoa_gap::ConstructValidityReport =
        serde_json::from_str(fixture).expect("fixture parses");
    assert_eq!(
        from_disk, artifact,
        "fixture must match the pipeline output"
    );

    let roundtrip: aoa_gap::ConstructValidityReport =
        serde_json::from_str(&serde_json::to_string(&artifact).unwrap()).unwrap();
    assert_eq!(roundtrip, artifact);
}
