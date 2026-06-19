use serde::{Deserialize, Serialize};

/// An external outcome a metric can be correlated against to earn gating status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalOutcome {
    /// Post-merge revert rate.
    RevertRate,
    /// Production incident count.
    IncidentCount,
    /// Human review-acceptance rate.
    ReviewAcceptance,
}

/// A single tie between a metric and one external outcome. `positive` records
/// whether the correlation was actually established (and in the right direction);
/// a reported-but-non-positive correlation does not earn gating.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OutcomeCorrelation {
    pub outcome: ExternalOutcome,
    pub positive: bool,
}

/// A construct-validity report tying a metric to one or more external outcomes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrelationReport {
    pub metric: String,
    pub correlations: Vec<OutcomeCorrelation>,
}

impl CorrelationReport {
    /// Whether at least one external outcome was positively correlated.
    pub fn has_positive_correlation(&self) -> bool {
        self.correlations.iter().any(|c| c.positive)
    }
}

/// Whether a metric may gate a decision or is advisory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricMode {
    Advisory,
    Gating,
}

/// Classify a metric as gating or advisory.
///
/// A metric is `Gating` only when a correlation report is supplied that ties it
/// to at least one positive external outcome; without that evidence it is
/// `Advisory`, regardless of how it looks in isolation (R9c construct validity).
pub fn classify_metric(_metric: &str, correlation: Option<&CorrelationReport>) -> MetricMode {
    match correlation {
        Some(report) if report.has_positive_correlation() => MetricMode::Gating,
        _ => MetricMode::Advisory,
    }
}
