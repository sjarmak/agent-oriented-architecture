use serde::{Deserialize, Serialize};

/// An external outcome a metric can be correlated against to earn gating status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalOutcome {
    /// Post-merge revert rate. Lower is better.
    RevertRate,
    /// Production incident count. Lower is better.
    IncidentCount,
    /// Human review-acceptance rate. Higher is better.
    ReviewAcceptance,
}

impl ExternalOutcome {
    /// Whether a HIGHER value of this outcome corresponds to BETTER real-world
    /// code. Reverts and incidents are harms (lower is better); review
    /// acceptance is a good (higher is better). Combined with a metric's own
    /// orientation, this fixes the sign a *confirming* correlation must have.
    fn higher_is_better(self) -> bool {
        matches!(self, ExternalOutcome::ReviewAcceptance)
    }
}

/// Which direction of a metric reads as "better" code. Required to interpret a
/// correlation's sign: the same external outcome confirms construct validity
/// with opposite signs depending on whether more of the metric is good
/// (`edit_locality`) or bad (`mutation_surface`, `reward_hacking_gap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricOrientation {
    HigherIsBetter,
    LowerIsBetter,
}

impl MetricOrientation {
    fn higher_is_better(self) -> bool {
        matches!(self, MetricOrientation::HigherIsBetter)
    }
}

/// A single tie between a metric and one external outcome, carrying the signed
/// coefficient (sign + magnitude), the sample size behind it, and the exact
/// two-sided permutation p-value. A bare "positive" flag is deliberately absent:
/// whether a correlation *confirms* validity depends on the metric's
/// orientation and the gating thresholds, evaluated by [`OutcomeCorrelation::is_confirming`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OutcomeCorrelation {
    pub outcome: ExternalOutcome,
    /// Signed rank-correlation coefficient in `[-1.0, 1.0]`.
    pub coefficient: f64,
    /// Number of paired observations backing the coefficient.
    pub n: usize,
    /// Exact two-sided permutation p-value.
    pub p_value: f64,
}

impl OutcomeCorrelation {
    /// The coefficient sign a confirming correlation must have, given the
    /// metric's orientation: positive when metric-good and outcome-good point
    /// the same way, negative when they oppose.
    fn confirming_is_positive(&self, orientation: MetricOrientation) -> bool {
        orientation.higher_is_better() == self.outcome.higher_is_better()
    }

    /// Whether this correlation is strong enough, in the right direction, and
    /// unlikely enough to be noise to count as evidence for gating, under the
    /// supplied thresholds.
    pub fn is_confirming(
        &self,
        orientation: MetricOrientation,
        thresholds: &GatingThresholds,
    ) -> bool {
        let sign_ok = if self.confirming_is_positive(orientation) {
            self.coefficient > 0.0
        } else {
            self.coefficient < 0.0
        };
        sign_ok
            && self.coefficient.abs() >= thresholds.min_magnitude
            && self.n >= thresholds.min_n
            && self.p_value <= thresholds.max_p_value
    }
}

/// Explicit, inspectable thresholds a correlation must clear to gate. Carried as
/// data — not hidden constants — so the gating preconditions are auditable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GatingThresholds {
    /// Minimum `|coefficient|`: a correlation weaker than this is too small to
    /// gate even if statistically significant.
    pub min_magnitude: f64,
    /// Minimum sample size: fewer observations cannot earn gating regardless of
    /// coefficient.
    pub min_n: usize,
    /// Maximum two-sided p-value (alpha): the correlation must be this unlikely
    /// under the no-association null.
    pub max_p_value: f64,
}

impl Default for GatingThresholds {
    /// Moderate effect (`|rho| >= 0.3`), at least 5 observations, significance
    /// at alpha = 0.05. Documented defaults, overridable per call.
    fn default() -> Self {
        Self {
            min_magnitude: 0.3,
            min_n: 5,
            max_p_value: 0.05,
        }
    }
}

/// A construct-validity report tying a metric to its external-outcome
/// correlations. The metric is `Gating` only if at least one correlation is
/// confirming under the gating thresholds; otherwise it stays `Advisory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrelationReport {
    pub metric: String,
    pub orientation: MetricOrientation,
    pub correlations: Vec<OutcomeCorrelation>,
}

impl CorrelationReport {
    /// Whether at least one external-outcome correlation confirms validity
    /// under the supplied thresholds (right direction, sufficient magnitude,
    /// sufficient sample, significant).
    pub fn has_confirming_correlation(&self, thresholds: &GatingThresholds) -> bool {
        self.correlations
            .iter()
            .any(|c| c.is_confirming(self.orientation, thresholds))
    }
}

/// Whether a metric may gate a decision or is advisory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricMode {
    Advisory,
    Gating,
}

/// Classify a metric as gating or advisory under R9c construct validity.
///
/// A metric is `Gating` only when a correlation report is supplied whose
/// evidence clears the gating thresholds — a confirming correlation to at least
/// one external outcome with the right sign, sufficient magnitude, sufficient
/// sample size, and significance. Without that evidence (no report, or one that
/// falls short on any axis) it is `Advisory`, regardless of how the metric looks
/// in isolation.
pub fn classify_metric(
    correlation: Option<&CorrelationReport>,
    thresholds: &GatingThresholds,
) -> MetricMode {
    match correlation {
        Some(report) if report.has_confirming_correlation(thresholds) => MetricMode::Gating,
        _ => MetricMode::Advisory,
    }
}

/// The gating-candidate metrics and their orientations — every metric that R9c
/// could let gate a generative feature once a confirming correlation exists.
/// `mutation_surface` and `reward_hacking_gap` are harms (smaller is better);
/// the rest are goods.
pub const GATING_CANDIDATES: &[(&str, MetricOrientation)] = &[
    ("retrieval_locality", MetricOrientation::HigherIsBetter),
    ("edit_locality", MetricOrientation::HigherIsBetter),
    (
        "invariant_discoverability",
        MetricOrientation::HigherIsBetter,
    ),
    ("mutation_surface", MetricOrientation::LowerIsBetter),
    ("budget_adherence", MetricOrientation::HigherIsBetter),
    ("reward_hacking_gap", MetricOrientation::LowerIsBetter),
];

/// One metric's classification within a [`ConstructValidityReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricClassification {
    pub metric: String,
    pub orientation: MetricOrientation,
    pub correlations: Vec<OutcomeCorrelation>,
    pub mode: MetricMode,
}

/// The construct-validity artifact: the data source consulted, the thresholds
/// applied, and the resulting per-metric classification. Reproducible by
/// re-running the pipeline over the same source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstructValidityReport {
    pub data_source: String,
    pub thresholds: GatingThresholds,
    pub metrics: Vec<MetricClassification>,
}

/// Build a construct-validity artifact: classify each supplied per-metric
/// correlation report under `thresholds`, recording the `data_source` so the
/// result is reproducible and its provenance inspectable.
pub fn build_report(
    data_source: impl Into<String>,
    reports: &[CorrelationReport],
    thresholds: &GatingThresholds,
) -> ConstructValidityReport {
    let metrics = reports
        .iter()
        .map(|r| MetricClassification {
            metric: r.metric.clone(),
            orientation: r.orientation,
            correlations: r.correlations.clone(),
            mode: classify_metric(Some(r), thresholds),
        })
        .collect();
    ConstructValidityReport {
        data_source: data_source.into(),
        thresholds: *thresholds,
        metrics,
    }
}

/// The documented data source consulted for the current determination, and the
/// reason it yields no external-outcome correlations.
pub const NO_EXTERNAL_OUTCOME_SOURCE: &str = "codeprobe run history (runs/codeprobe-*): \
no post-merge revert, production-incident, or human review-acceptance fields are recorded. \
The only per-trial outcome is the oracle pass/reward, which is conditioned on held-out success \
and is therefore circular for construct validity. ground_truth_commit anchors the oracle but \
correlating a metric with oracle agreement is the same tautology. No external-outcome corpus is \
available as of 2026-06-20, so every gating candidate stays advisory.";

/// The current R9c determination: with no external-outcome corpus available,
/// every gating candidate has no confirming correlation and is `Advisory`. The
/// returned artifact names the data source it consulted and is reproducible by
/// re-running this function — the executable form of "no metric gates a feature
/// until real external correlation exists".
pub fn current_determination() -> ConstructValidityReport {
    let reports: Vec<CorrelationReport> = GATING_CANDIDATES
        .iter()
        .map(|(metric, orientation)| CorrelationReport {
            metric: (*metric).to_string(),
            orientation: *orientation,
            correlations: Vec::new(),
        })
        .collect();
    build_report(
        NO_EXTERNAL_OUTCOME_SOURCE,
        &reports,
        &GatingThresholds::default(),
    )
}
