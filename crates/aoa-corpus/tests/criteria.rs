//! The one construct-validity criterion that also exercises the corpus-side
//! Spearman pipeline. It lives here rather than in `aoa-construct` because that
//! crate does not depend on `aoa-corpus` — and giving it a dev-dependency purely
//! for this test would invert the intended direction of the split.

use aoa_construct::{
    classify_metric, CorrelationReport, ExternalOutcome, GatingThresholds, MetricMode,
    MetricOrientation, OutcomeCorrelation,
};
use aoa_corpus::spearman;

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
