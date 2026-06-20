use std::collections::BTreeSet;

use aoa_gap::HeldOutProvenance;

use crate::provenance::classify_provenance;

/// One independently-mined backend's accepted answer, with its identity.
///
/// codeprobe's consensus mining runs several backends and records each one's
/// file-set in `divergence_report.json`. Carrying the `backend` name alongside
/// the `files` is what makes `NativeComposed` auditable: a reviewer can see
/// *which* backends agreed, not just how many distinct file-sets resulted. Two
/// backends that produced the *same* files still count as two independent runs —
/// identical agreement is the strongest independence signal, not a collapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedSolution {
    /// Backend identity from `divergence_report.json` (e.g. `ast`, `treesitter`).
    pub backend: String,
    /// The file-set this backend independently mined as the answer.
    pub files: BTreeSet<String>,
}

/// A codeprobe-mined task mapped into AOA task inputs.
///
/// codeprobe mines tasks from repo history, so the held-out leg is composed
/// externally (from a real ground-truth commit) or natively (by multi-backend
/// consensus), never synthesized toolkit-side from the visible spec. This struct
/// carries exactly the facts the AOA gap and metric gates need: the instruction,
/// the oracle's expected file set `G_t`, the per-backend accepted solutions (with
/// identity) recorded by consensus mining, and the commit used to classify
/// held-out provenance.
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
    /// One entry per independently-mined backend that agreed in consensus mining.
    /// Two or more distinct backend identities make the task `NativeComposed`.
    pub accepted_solutions: Vec<AcceptedSolution>,
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

    /// The distinct, non-empty accepted-solution file-sets, for edit-locality
    /// anchors.
    ///
    /// Backends that mined the *same* files collapse to one entry, and empty
    /// file-sets are dropped: edit locality measures the spread between genuinely
    /// different accepted solutions, so identical or empty sets carry no
    /// floor/ceiling information. (Those backends still count as independent runs
    /// for `NativeComposed` — that judgment is about provenance, not edit spread.)
    /// Deterministically ordered.
    pub fn accepted_solution_files(&self) -> Vec<BTreeSet<String>> {
        self.accepted_solutions
            .iter()
            .filter(|s| !s.files.is_empty())
            .map(|s| s.files.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
