use aoa_gap::{
    classify_metric, compare, compute_gap, CanaryItem, CorrelationReport, ExternalOutcome,
    GapError, GapOutcome, HeldOutProvenance, Label, MetricMode, OutcomeCorrelation, RunResult,
    TaskOutcome,
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

// Criterion 6: a metric is advisory without a positive correlation report; only
// a metric WITH a supplied positive correlation may be gating.
#[test]
fn criterion_6_construct_validity() {
    // No report at all -> advisory.
    assert_eq!(
        classify_metric("edit_locality", None),
        MetricMode::Advisory
    );

    // Report present but no positive correlation -> advisory.
    let negative = CorrelationReport {
        metric: "edit_locality".into(),
        correlations: vec![OutcomeCorrelation {
            outcome: ExternalOutcome::RevertRate,
            positive: false,
        }],
    };
    assert_eq!(
        classify_metric("edit_locality", Some(&negative)),
        MetricMode::Advisory
    );

    // At least one positive external outcome -> gating.
    for outcome in [
        ExternalOutcome::RevertRate,
        ExternalOutcome::IncidentCount,
        ExternalOutcome::ReviewAcceptance,
    ] {
        let report = CorrelationReport {
            metric: "edit_locality".into(),
            correlations: vec![OutcomeCorrelation {
                outcome,
                positive: true,
            }],
        };
        assert_eq!(
            classify_metric("edit_locality", Some(&report)),
            MetricMode::Gating
        );
    }
}
