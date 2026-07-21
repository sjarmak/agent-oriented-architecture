//! Construct validity (R9c) for the AOA Toolkit.
//!
//! A metric is `advisory` until a correlation report ties it to at least one
//! external outcome (revert rate, incident count, or review acceptance) with the
//! right sign, sufficient magnitude, sufficient sample size, and significance (an
//! exact permutation p-value); only then may it be `gating`. With no
//! external-outcome corpus presently available, every gating candidate stays
//! advisory — see [`current_determination`].
//!
//! This crate holds the classification rules and the behavioral signal that
//! qualifies a determination when held-out observations are too few. It depends
//! on nothing: mining the outcomes that could confirm a correlation is the job of
//! `aoa-corpus`, which depends on this crate rather than the reverse.

mod construct;
mod signal;

pub use construct::{
    build_report, classify_metric, current_determination, ConstructValidityReport,
    CorrelationReport, ExternalOutcome, GatingThresholds, MetricClassification, MetricMode,
    MetricName, MetricOrientation, OutcomeCorrelation, GATING_CANDIDATES,
    NO_EXTERNAL_OUTCOME_SOURCE, STRUCTURE_MEASURE_SPECS,
};
pub use signal::{
    determination_with_signal, BehavioralSignal, InsufficientDataNote, BEHAVIORAL_METRICS,
    INSUFFICIENT_DATA_REASON, MIN_HELD_OUT_OBSERVATIONS,
};
