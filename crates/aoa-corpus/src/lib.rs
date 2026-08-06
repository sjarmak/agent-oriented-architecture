//! External-outcome corpus for the AOA Toolkit.
//!
//! Construct validity promotes a metric from `advisory` to `gating` only against
//! real external outcomes. This crate produces them: it mines revert history out
//! of a git clone, scores a repo against the Factory checkbox-baseline rubric,
//! and correlates either against a metric with an exact Spearman permutation
//! test. The joined result is a [`CorpusReport`].
//!
//! **Layer: measurement.** Mining reverts and scoring a rubric look like
//! capture, but neither is produced for its own sake: both exist to feed the
//! join, and the join is this crate's one reason to change. So it sits above
//! `aoa-construct` — it depends on that crate for the report it fills in, and
//! `aoa-construct` knows nothing about corpora. The edge is one-way on purpose;
//! reversing it would put revert mining and the Factory rubric underneath
//! `aoa-audit` and `aoa-recommend`, which consume `aoa-construct` as a leaf and
//! have no use for either.
//!
//! It deliberately does not depend on `aoa-gap`: mining reverts has nothing to
//! do with computing a held-out gap.
//!
//! Offline, not CLI-side. Nothing here clones, shells git, or reaches the
//! network — a live corpus is assembled by the app-layer driver in
//! `aoa::commands::corpus`, which injects a real [`GitRunner`] into the
//! otherwise-offline miner. The apparatus itself is a library concern; only the
//! subprocess half is the binary's. Nothing here is on the gating path today:
//! with no corpus available every candidate stays advisory.

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
