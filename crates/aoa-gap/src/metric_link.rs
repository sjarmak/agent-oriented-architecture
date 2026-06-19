use aoa_metrics::MetricRecord;

use crate::construct::{classify_metric, CorrelationReport, MetricMode};

/// Classify how an `aoa_metrics` record may be used downstream.
///
/// A locality `MetricRecord` from `aoa-metrics` is advisory on its own: it
/// measures structural locality, not an external outcome. It may gate a
/// migration decision only once a construct-validity correlation report ties the
/// named metric to a positive external outcome (R9c). This is the bridge the
/// gap layer offers callers that already hold metric records.
pub fn classify_record(
    metric: &str,
    _record: &MetricRecord,
    correlation: Option<&CorrelationReport>,
) -> MetricMode {
    classify_metric(metric, correlation)
}
