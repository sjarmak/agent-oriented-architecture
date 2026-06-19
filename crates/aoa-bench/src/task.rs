use std::collections::BTreeSet;

use aoa_gap::HeldOutProvenance;

use crate::provenance::classify_provenance;

/// A codeprobe-mined task mapped into AOA task inputs.
///
/// codeprobe mines tasks from repo history, so the held-out leg is composed
/// externally (from a real ground-truth commit) or natively (by multi-backend
/// consensus), never synthesized toolkit-side from the visible spec. This struct
/// carries exactly the facts the AOA gap and metric gates need: the instruction,
/// the oracle's expected file set `G_t`, the independently-mined accepted-solution
/// file-sets, and the structural evidence used to classify held-out provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeprobeTask {
    /// Stable task id (e.g. `probe-returntype-000`).
    pub id: String,
    /// Repo the task was mined from.
    pub repo: String,
    /// The natural-language instruction the agent is given.
    pub instruction: String,
    /// The oracle's expected file set — the gold artifacts `G_t`.
    pub oracle_files: BTreeSet<String>,
    /// The repo-history commit the oracle was mined against, if any. Its presence
    /// is what makes a `file_list` oracle externally composed (contamination-free).
    pub ground_truth_commit: Option<String>,
    /// One file-set per independently-mined backend that agreed on the answer.
    /// Two or more distinct sets supply the accepted-solutions edit-locality needs.
    pub accepted_solutions: Vec<BTreeSet<String>>,
}

impl CodeprobeTask {
    /// Classify how this task's held-out leg was composed.
    ///
    /// The decision is a deterministic predicate over structural facts:
    /// a `file_list` oracle anchored to a real ground-truth commit is `External`;
    /// agreement among two or more mined backends is `NativeComposed`; a task with
    /// neither independent leg is `None`. It is never `SynthesizedFromVisible`,
    /// because codeprobe never derives the held-out answer from the visible spec.
    pub fn held_out_provenance(&self) -> HeldOutProvenance {
        classify_provenance(self)
    }
}
