use aoa_gap::HeldOutProvenance;
use aoa_metrics::Confidence;

use crate::types::Eligibility;

/// Whether a repo may vote in R0.
///
/// A repo votes ONLY when it is high-confidence (SCIP-grade) AND native-span
/// (its held-out suite is natively composed) AND calibrated. Any single failure
/// excludes it, per R-silent; an ineligible repo contributes no vote.
pub fn is_eligible(e: &Eligibility) -> bool {
    matches!(e.confidence, Confidence::High)
        && matches!(e.native_span, HeldOutProvenance::NativeComposed)
        && e.calibrated
}
