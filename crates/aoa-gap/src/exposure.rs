use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Stable identity for one held-out question against a pinned repository state.
///
/// Codeprobe task ids are intentionally absent: mining may rename a task when
/// its evidence requirements change without changing the repository question
/// whose answer has already been exposed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectKey {
    /// Stable repository identifier from codeprobe's task/campaign metadata.
    pub repo_id: String,
    /// Exact source revision against which the question was mined.
    pub baseline_commit: String,
    /// Structured instruction-heading subject, independent of the task id.
    pub oracle_target_symbol: String,
    /// Question template/family, such as `dependency_analysis`.
    pub question_family: String,
}

/// Whether an admitted held-out subject set has already been spent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureStatus {
    /// No admitted subject has a persisted trial artifact.
    Unexposed,
    /// Some, but not all, admitted subjects have persisted trial artifacts.
    PartiallyExposed { subjects: BTreeSet<SubjectKey> },
    /// Every admitted subject has a persisted trial artifact.
    Exposed,
}

impl ExposureStatus {
    pub fn is_unexposed(&self) -> bool {
        matches!(self, Self::Unexposed)
    }
}
