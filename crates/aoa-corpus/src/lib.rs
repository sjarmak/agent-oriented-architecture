//! External-outcome corpus for the AOA Toolkit.
//!
//! Construct validity promotes a metric from `advisory` to `gating` only against
//! real external outcomes. This crate produces them: it mines revert history out
//! of a git clone, scores a repo against the Factory checkbox-baseline rubric,
//! and correlates either against a metric with an exact Spearman permutation
//! test. The joined result is a [`CorpusReport`].
//!
//! Nothing here is on the gating path today — with no corpus available every
//! candidate stays advisory — so this crate is a CLI-side concern. It depends on
//! `aoa-construct` for the report it fills in, and deliberately not on `aoa-gap`:
//! mining reverts has nothing to do with computing a held-out gap.

mod checkbox_baseline;
mod correlation;
mod outcome;

pub use checkbox_baseline::{
    score_repo, CheckboxBaseline, CheckboxBaselineError, CriterionResult, CriterionStatus,
    LevelScore, Pillar, PillarScore, FACTORY_CRITERIA_SOURCE, FACTORY_CRITERIA_VERSION,
    FACTORY_LEVEL_NAMES, PILLARS,
};
pub use correlation::{spearman, CorrelationError, RankCorrelation, MAX_EXACT_N};
pub use outcome::{
    build_report_from_corpus, mine_reverts, parse_reverted_shas, revert_log_command,
    CheckboxBaselineCorrelation, Corpus, CorpusJoinError, CorpusMetricError, CorpusReport,
    GitRunner, MinedCommit, Repo, RevertMinerError,
};
