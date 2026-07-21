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
//! observations available for one repo. Below the [`MIN_HELD_OUT_OBSERVATIONS`]
//! window the behavioral metrics are tagged [`MetricMode::InsufficientData`] —
//! a state DISTINCT from `Advisory`: an Advisory metric was computed but has
//! not earned external validation; an InsufficientData metric cannot be
//! computed at all, with reason [`INSUFFICIENT_DATA_REASON`]. Once enough
//! observations accumulate, [`determination_with_signal`] returns the ordinary
//! determination and the metrics light up (as Advisory, until R9c correlation
//! promotes them).

use serde::{Deserialize, Serialize};

use crate::construct::{
    current_determination, ConstructValidityReport, MetricClassification, MetricMode, MetricName,
};

/// Held-out behavioral observations one repo must supply before its behavioral
/// metrics are computed at all.
///
/// A calibration floor, not a derived quantity: R9c has not run, so no external
/// outcome yet says how much held-out signal a locality metric needs to stop
/// reporting session noise as repo structure. It sits above the correlation-side
/// floor ([`crate::GatingThresholds::min_n`] = 5) and low enough that a repo
/// under real observation reaches it. Moving it is a calibration decision, not a
/// refactor.
///
/// Deliberately NOT [`crate::MAX_EXACT_N`], which happens to hold the same
/// value: that is a *ceiling* on permutation compute over a population of repos,
/// this is a *floor* on sessions within one repo. Coupled, the sampled
/// significance test `correlation.rs` anticipates would raise the cap and
/// thereby make the behavioral window *harder* to satisfy.
pub const MIN_HELD_OUT_OBSERVATIONS: usize = 10;

/// The gating-candidate metrics that are *behavioral* — computable only from
/// held-out task traces, never from repo structure alone. A subset of
/// [`crate::GATING_CANDIDATES`] by type: every [`MetricName`] is a registered
/// candidate by construction.
pub const BEHAVIORAL_METRICS: &[MetricName] = &[
    MetricName::RetrievalLocality,
    MetricName::EditLocality,
    MetricName::InvariantDiscoverability,
    MetricName::MutationSurface,
];

/// The reason stamped on every behavioral metric tagged
/// [`MetricMode::InsufficientData`].
pub const INSUFFICIENT_DATA_REASON: &str = "no held-out behavioral signal for this repo yet";

/// Whether `metric` (a wire name) names a member of the behavioral family.
fn is_behavioral(metric: &str) -> bool {
    BEHAVIORAL_METRICS.iter().any(|b| b.as_str() == metric)
}

/// The count of held-out behavioral observations available for one repo, as
/// supplied by the caller. Purely mechanical, no judgment. Today two producers
/// count: the audit counts observe-captured sessions that carry a landed edit
/// (`aoa_observe_shim::TraceCorpus::observations`), and `eval run` counts its
/// per-trial records. No code path counts codeprobe-mineable tasks yet — a
/// repo with rich history but no observe corpus reports zero observations
/// (which is why [`INSUFFICIENT_DATA_REASON`] says "held-out behavioral
/// signal", not "history").
///
/// `required` is carried as data (inspectable-thresholds discipline) and is
/// [`MIN_HELD_OUT_OBSERVATIONS`]: below that window a repo cannot supply enough
/// held-out signal for its behavioral metrics to mean anything. It is written
/// to the wire but never read back from it — see the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehavioralSignal {
    /// Held-out behavioral observations counted for the repo.
    pub observations: usize,
    /// Observations needed before the behavioral metrics light up.
    ///
    /// Serialized for inspection, never deserialized: this is the *reader's*
    /// calibration floor, not producer data, so it is re-derived from
    /// [`MIN_HELD_OUT_OBSERVATIONS`] on every ingest. Trusting the wire here
    /// let a hand-edited `{"observations":1,"required":1}` report a greenfield
    /// repo as sufficient and light up all four locality metrics, since
    /// [`is_sufficient`](Self::is_sufficient) compares against exactly this
    /// field (aoa-d6t.38).
    ///
    /// Re-deriving rather than rejecting a mismatch is deliberate:
    /// [`MIN_HELD_OUT_OBSERVATIONS`] is a calibration floor expected to move,
    /// and rejecting would turn each move into a data-compat break for reports
    /// written under the previous floor.
    ///
    /// This takes the *threshold* out of an editor's reach; it does not make
    /// `observations` tamper-proof. That count is a measurement, and no reader
    /// can validate it without re-measuring — but a forged count now has to
    /// claim a full window of held-out signal the repo visibly lacks, instead
    /// of quietly lowering the bar to meet it.
    #[serde(skip_deserializing, default = "required_window")]
    pub required: usize,
}

/// The ingest-time value of [`BehavioralSignal::required`]. A function because
/// `serde(default = ...)` takes a path, not a constant.
fn required_window() -> usize {
    MIN_HELD_OUT_OBSERVATIONS
}

impl BehavioralSignal {
    /// A signal of `observations` against the [`MIN_HELD_OUT_OBSERVATIONS`]
    /// window.
    pub fn from_observations(observations: usize) -> Self {
        Self {
            observations,
            required: MIN_HELD_OUT_OBSERVATIONS,
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
            metrics: BEHAVIORAL_METRICS.iter().map(|m| m.to_string()).collect(),
            reason: INSUFFICIENT_DATA_REASON.to_string(),
        }
    }

    /// The one-line human rendering of the note against its signal, shared by
    /// every surface (audit, eval) that reports the withheld metrics alongside
    /// the observation count.
    #[must_use]
    pub fn render_line(&self, signal: &BehavioralSignal) -> String {
        format!(
            "behavioral metrics [InsufficientData]: {} — {} ({} of {} observation(s))",
            self.metrics.join(", "),
            self.reason,
            signal.observations,
            signal.required,
        )
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
            if is_behavioral(&m.metric) {
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

    /// The wire names of the behavioral family, for comparing against the
    /// `Vec<String>` a note carries.
    fn behavioral_names() -> Vec<String> {
        BEHAVIORAL_METRICS.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn behavioral_family_is_exactly_the_four_locality_metrics() {
        // Membership in GATING_CANDIDATES is type-level (every MetricName is a
        // registered candidate); the family SIZE is policy and stays asserted.
        assert_eq!(
            BEHAVIORAL_METRICS.len(),
            4,
            "exactly the 4 locality metrics"
        );
    }

    #[test]
    fn signal_window_is_the_held_out_minimum() {
        let below = BehavioralSignal::from_observations(MIN_HELD_OUT_OBSERVATIONS - 1);
        assert!(!below.is_sufficient());
        assert_eq!(below.required, MIN_HELD_OUT_OBSERVATIONS);

        let at = BehavioralSignal::from_observations(MIN_HELD_OUT_OBSERVATIONS);
        assert!(at.is_sufficient());
        assert!(at.insufficient_data().is_none());

        let note = below.insufficient_data().expect("below window");
        assert_eq!(note.reason, INSUFFICIENT_DATA_REASON);
        assert_eq!(note.metrics, behavioral_names());
    }

    /// Pins this file's own constant only. Asserting anything about
    /// `MAX_EXACT_N` here would re-create, in test code, the cross-module
    /// reference the split just removed from the production path.
    #[test]
    fn window_value_is_pinned_as_a_calibration_decision() {
        assert_eq!(
            MIN_HELD_OUT_OBSERVATIONS, 10,
            "calibration floor; moving it is a decision, not a refactor"
        );
    }

    #[test]
    fn insufficient_signal_tags_behavioral_metrics_not_structural() {
        let report = determination_with_signal(&BehavioralSignal::from_observations(0));
        assert_eq!(report.metrics.len(), GATING_CANDIDATES.len());
        for m in &report.metrics {
            if is_behavioral(&m.metric) {
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
        let report = determination_with_signal(&BehavioralSignal::from_observations(
            MIN_HELD_OUT_OBSERVATIONS,
        ));
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
        assert_eq!(note.metrics, behavioral_names());
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

    /// aoa-d6t.38: `required` is the reader's own calibration constant, not
    /// producer data. A hand-edited report that lowers it must not be able to
    /// declare a repo with one observation sufficient — that would defeat the
    /// precondition outright, since `is_sufficient` compares against exactly
    /// this field.
    #[test]
    fn tampered_required_cannot_forge_sufficient_signal() {
        let forged: BehavioralSignal =
            serde_json::from_str(r#"{"observations":1,"required":1}"#).expect("deserializes");

        assert_eq!(
            forged.required, MIN_HELD_OUT_OBSERVATIONS,
            "required is re-derived on ingest, never read from the wire"
        );
        assert!(!forged.is_sufficient());
        assert!(forged.insufficient_data().is_some());

        // The impact the precondition exists to prevent: the four locality
        // metrics must stay InsufficientData under the forged signal.
        let report = determination_with_signal(&forged);
        for m in report.metrics.iter().filter(|m| is_behavioral(&m.metric)) {
            assert_eq!(
                m.mode,
                MetricMode::InsufficientData,
                "{} must not light up on a forged window",
                m.metric
            );
        }
    }

    /// Re-deriving on ingest must not change what honest producers write or
    /// read back: the threshold stays on the wire for inspection, and a
    /// genuine signal round-trips to itself.
    #[test]
    fn honest_signal_round_trips_with_the_threshold_still_on_the_wire() {
        let signal = BehavioralSignal::from_observations(3);
        let json = serde_json::to_string(&signal).expect("serialize");

        assert!(
            json.contains(&format!("\"required\":{MIN_HELD_OUT_OBSERVATIONS}")),
            "producers still emit the threshold for inspection: {json}"
        );
        assert_eq!(
            serde_json::from_str::<BehavioralSignal>(&json).expect("round-trips"),
            signal
        );
    }
}
