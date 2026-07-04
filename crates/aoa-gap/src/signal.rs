//! Greenfield/cold-start behavioral-signal precondition (aoa-d6t.23).
//!
//! The four locality metrics are *behavioral*: they are computed from held-out
//! task traces (mined by codeprobe from past commits, or captured live by
//! `aoa observe`). A repo with too little history yields no mined tasks, no
//! held-out set, and no traces — so none of them is computable. Before this
//! precondition existed, the toolkit silently degraded to the checkbox baseline
//! and misrepresented greenfield repos.
//!
//! [`BehavioralSignal`] is the mechanical count of held-out behavioral
//! observations available for one repo. Below the [`MAX_EXACT_N`] window the
//! behavioral metrics are tagged [`MetricMode::InsufficientData`] — a state
//! DISTINCT from `Advisory`: an Advisory metric was computed but has not earned
//! external validation; an InsufficientData metric cannot be computed at all,
//! with reason [`INSUFFICIENT_DATA_REASON`]. Once enough observations
//! accumulate, [`determination_with_signal`] returns the ordinary
//! determination and the metrics light up (as Advisory, until R9c correlation
//! promotes them).

use serde::{Deserialize, Serialize};

use crate::construct::{
    current_determination, ConstructValidityReport, MetricClassification, MetricMode,
};
use crate::correlation::MAX_EXACT_N;

/// The gating-candidate metrics that are *behavioral* — computable only from
/// held-out task traces, never from repo structure alone. A subset of
/// [`crate::GATING_CANDIDATES`] (asserted in tests).
pub const BEHAVIORAL_METRICS: &[&str] = &[
    "retrieval_locality",
    "edit_locality",
    "invariant_discoverability",
    "mutation_surface",
];

/// The reason stamped on every behavioral metric tagged
/// [`MetricMode::InsufficientData`].
pub const INSUFFICIENT_DATA_REASON: &str = "no held-out behavioral signal for this repo yet";

/// The count of held-out behavioral observations available for one repo:
/// codeprobe-mined tasks plus observe-captured live-session traces. Purely
/// mechanical — a file/trial count, no judgment.
///
/// `required` is carried as data (inspectable-thresholds discipline) and is
/// [`MAX_EXACT_N`]: below the exact-permutation window a repo cannot supply
/// enough held-out signal for its behavioral metrics to mean anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehavioralSignal {
    /// Held-out behavioral observations counted for the repo.
    pub observations: usize,
    /// Observations needed before the behavioral metrics light up.
    pub required: usize,
}

impl BehavioralSignal {
    /// A signal of `observations` against the [`MAX_EXACT_N`] window.
    pub fn from_observations(observations: usize) -> Self {
        Self {
            observations,
            required: MAX_EXACT_N,
        }
    }

    /// Whether the repo has enough held-out signal for behavioral metrics.
    pub fn is_sufficient(&self) -> bool {
        self.observations >= self.required
    }

    /// The insufficient-data note for reports, `None` when sufficient.
    pub fn insufficient_data(&self) -> Option<InsufficientDataNote> {
        (!self.is_sufficient()).then(InsufficientDataNote::behavioral)
    }
}

impl Default for BehavioralSignal {
    /// Zero observations — the honest default for a report deserialized from a
    /// producer that predates the behavioral-signal precondition: absent a
    /// recorded count, no held-out signal may be assumed.
    fn default() -> Self {
        Self::from_observations(0)
    }
}

/// The report block naming which metrics are [`MetricMode::InsufficientData`]
/// and why. Carried on audit/eval/recommend surfaces so the state is explicit
/// in both registers, never inferred from a missing score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsufficientDataNote {
    /// The affected metric names.
    pub metrics: Vec<String>,
    /// Why the metrics carry no score.
    pub reason: String,
}

impl InsufficientDataNote {
    /// The note for the behavioral metric family with the canonical reason.
    pub fn behavioral() -> Self {
        Self {
            metrics: BEHAVIORAL_METRICS
                .iter()
                .map(|m| (*m).to_string())
                .collect(),
            reason: INSUFFICIENT_DATA_REASON.to_string(),
        }
    }
}

/// The R9c determination conditioned on a repo's behavioral signal.
///
/// With sufficient signal this is exactly [`current_determination`]. Below the
/// window, every behavioral candidate is re-tagged
/// [`MetricMode::InsufficientData`] with [`INSUFFICIENT_DATA_REASON`] — never
/// left `Advisory` (which would claim the metric was computed) and never handed
/// a fabricated score. Structural candidates are unaffected: they are measured
/// from the repo tree and need no traces.
pub fn determination_with_signal(signal: &BehavioralSignal) -> ConstructValidityReport {
    let base = current_determination();
    if signal.is_sufficient() {
        return base;
    }
    let metrics = base
        .metrics
        .into_iter()
        .map(|m| {
            if BEHAVIORAL_METRICS.contains(&m.metric.as_str()) {
                MetricClassification {
                    mode: MetricMode::InsufficientData,
                    reason: Some(INSUFFICIENT_DATA_REASON.to_string()),
                    ..m
                }
            } else {
                m
            }
        })
        .collect();
    ConstructValidityReport { metrics, ..base }
}

impl ConstructValidityReport {
    /// The insufficient-data note derived from this determination: the metrics
    /// tagged [`MetricMode::InsufficientData`] and their recorded reason.
    /// `None` when every metric was classifiable. A row missing its reason
    /// (hand-built, not via [`determination_with_signal`]) reports the
    /// canonical [`INSUFFICIENT_DATA_REASON`].
    pub fn insufficient_data(&self) -> Option<InsufficientDataNote> {
        let rows: Vec<&MetricClassification> = self
            .metrics
            .iter()
            .filter(|m| m.mode == MetricMode::InsufficientData)
            .collect();
        if rows.is_empty() {
            return None;
        }
        let reason = rows
            .iter()
            .find_map(|m| m.reason.clone())
            .unwrap_or_else(|| INSUFFICIENT_DATA_REASON.to_string());
        Some(InsufficientDataNote {
            metrics: rows.iter().map(|m| m.metric.clone()).collect(),
            reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construct::GATING_CANDIDATES;

    #[test]
    fn behavioral_metrics_are_registered_gating_candidates() {
        let candidates: Vec<&str> = GATING_CANDIDATES.iter().map(|(m, _)| *m).collect();
        for metric in BEHAVIORAL_METRICS {
            assert!(
                candidates.contains(metric),
                "behavioral metric '{metric}' absent from GATING_CANDIDATES"
            );
        }
        assert_eq!(
            BEHAVIORAL_METRICS.len(),
            4,
            "exactly the 4 locality metrics"
        );
    }

    #[test]
    fn signal_window_is_max_exact_n() {
        let below = BehavioralSignal::from_observations(MAX_EXACT_N - 1);
        assert!(!below.is_sufficient());
        assert_eq!(below.required, MAX_EXACT_N);

        let at = BehavioralSignal::from_observations(MAX_EXACT_N);
        assert!(at.is_sufficient());
        assert!(at.insufficient_data().is_none());

        let note = below.insufficient_data().expect("below window");
        assert_eq!(note.reason, INSUFFICIENT_DATA_REASON);
        assert_eq!(note.metrics, BEHAVIORAL_METRICS);
    }

    #[test]
    fn insufficient_signal_tags_behavioral_metrics_not_structural() {
        let report = determination_with_signal(&BehavioralSignal::from_observations(0));
        assert_eq!(report.metrics.len(), GATING_CANDIDATES.len());
        for m in &report.metrics {
            if BEHAVIORAL_METRICS.contains(&m.metric.as_str()) {
                assert_eq!(
                    m.mode,
                    MetricMode::InsufficientData,
                    "{} must be insufficient-data, NOT advisory",
                    m.metric
                );
                assert_eq!(m.reason.as_deref(), Some(INSUFFICIENT_DATA_REASON));
            } else {
                assert_eq!(
                    m.mode,
                    MetricMode::Advisory,
                    "structural candidate {} needs no traces",
                    m.metric
                );
                assert_eq!(m.reason, None);
            }
        }
    }

    #[test]
    fn sufficient_signal_lights_the_metrics_up_as_advisory() {
        let report = determination_with_signal(&BehavioralSignal::from_observations(MAX_EXACT_N));
        assert_eq!(report, current_determination());
        for m in &report.metrics {
            assert_eq!(m.mode, MetricMode::Advisory, "{}", m.metric);
        }
        assert!(report.insufficient_data().is_none());
    }

    #[test]
    fn report_note_names_the_tagged_metrics_and_reason() {
        let report = determination_with_signal(&BehavioralSignal::from_observations(3));
        let note = report.insufficient_data().expect("insufficient");
        assert_eq!(note.metrics, BEHAVIORAL_METRICS);
        assert_eq!(note.reason, INSUFFICIENT_DATA_REASON);
    }

    #[test]
    fn insufficient_data_mode_round_trips_and_renders() {
        let report = determination_with_signal(&BehavioralSignal::from_observations(0));
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(
            json.contains("\"insufficient_data\""),
            "wire form carries the distinct state: {json}"
        );
        let back: ConstructValidityReport = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back, report);

        let rendered = report.render_human();
        assert!(
            rendered.contains("[InsufficientData] retrieval_locality"),
            "human register shows the state: {rendered}"
        );
        assert!(
            rendered.contains(INSUFFICIENT_DATA_REASON),
            "human register shows the reason: {rendered}"
        );
        assert!(rendered.contains("4 insufficient-data"), "{rendered}");
    }

    #[test]
    fn default_signal_assumes_no_observations() {
        let signal = BehavioralSignal::default();
        assert_eq!(signal.observations, 0);
        assert!(!signal.is_sufficient());
    }
}
