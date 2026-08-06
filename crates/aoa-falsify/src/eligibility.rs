use aoa_domain::HeldOutProvenance;
use aoa_trace::Confidence;

use crate::types::Eligibility;

/// Whether a repo may vote in R0.
///
/// A repo votes ONLY when it is high-confidence (SCIP-grade), has certified
/// held-out provenance (`External` or `NativeComposed`), AND is calibrated. Any
/// admitted subjects are unexposed. Any single failure excludes it, per
/// R-silent; an ineligible repo contributes no vote.
pub fn is_eligible(e: &Eligibility) -> bool {
    matches!(e.confidence, Confidence::High)
        && matches!(
            e.native_span,
            HeldOutProvenance::External | HeldOutProvenance::NativeComposed
        )
        && e.calibrated
        && e.exposure.is_unexposed()
}
