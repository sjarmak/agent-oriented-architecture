//! Reward-hacking gap, held-out integrity, and construct validity for the AOA Toolkit.
//!
//! This crate is the primary evaluation gate. It computes the visible-vs-held-out
//! success gap (R9) and decides whether an AOA migration is `good`: a migration
//! earns `good` ONLY when its held-out pass rate improves AND the gap holds or
//! reduces — never on a visible-pass plus locality improvement alone.
//!
//! Held-out integrity (R0b) is enforced two ways: a held-out suite synthesized
//! toolkit-side from the visible specs is rejected loudly, and an injected
//! leakage canary fails a comparison when the held-out rate rises without the
//! visible rate moving and a known held-out item flips against its expectation.
//! A benchmark with no native composed held-out suite yields `gap: unavailable`
//! and refuses to label any migration — gating on an absent gap is prohibited.
//!
//! Construct validity (R9c): a metric is `advisory` until a correlation report
//! ties it to at least one external outcome (revert rate, incident count, or
//! review acceptance) with the right sign, sufficient magnitude, sufficient
//! sample size, and significance (an exact permutation p-value); only then may
//! it be `gating`. With no external-outcome corpus presently available, every
//! gating candidate stays advisory — see [`current_determination`].
//!
//! All logic here is deterministic mechanism: rates are arithmetic means of
//! per-task booleans and labels are boolean predicates over rate/gap deltas.

mod checkbox_baseline;
mod compare;
mod construct;
mod correlation;
mod error;
mod gap;
mod outcome;
mod provenance;
mod run;
mod signal;

pub use checkbox_baseline::{
    score_repo, CheckboxBaseline, CheckboxBaselineError, CriterionResult, CriterionStatus,
    LevelScore, Pillar, PillarScore, FACTORY_CRITERIA_SOURCE, FACTORY_CRITERIA_VERSION,
    FACTORY_LEVEL_NAMES, PILLARS,
};
pub use compare::{compare, CompareOutcome, CompareWarning, Label};
pub use construct::{
    build_report, classify_metric, current_determination, ConstructValidityReport,
    CorrelationReport, ExternalOutcome, GatingThresholds, MetricClassification, MetricMode,
    MetricName, MetricOrientation, OutcomeCorrelation, GATING_CANDIDATES,
    NO_EXTERNAL_OUTCOME_SOURCE, STRUCTURE_MEASURE_SPECS,
};
pub use correlation::{spearman, CorrelationError, RankCorrelation, MAX_EXACT_N};
pub use error::GapError;
pub use gap::{compute_gap, GapOutcome};
pub use outcome::{
    build_report_from_corpus, mine_reverts, parse_reverted_shas, revert_log_command,
    CheckboxBaselineCorrelation, Corpus, CorpusJoinError, CorpusMetricError, CorpusReport,
    GitRunner, MinedCommit, Repo,
};
pub use provenance::HeldOutProvenance;
pub use run::{CanaryItem, RunResult, TaskOutcome};
pub use signal::{
    determination_with_signal, BehavioralSignal, InsufficientDataNote, BEHAVIORAL_METRICS,
    INSUFFICIENT_DATA_REASON, MIN_HELD_OUT_OBSERVATIONS,
};
