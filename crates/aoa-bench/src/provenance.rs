use std::collections::BTreeSet;

use aoa_gap::HeldOutProvenance;

use crate::task::CodeprobeTask;

/// Classify a task's held-out provenance from its mined structure.
///
/// codeprobe mines from repo history, so the only legitimate labels are
/// `External` (a `file_list` oracle anchored to a real ground-truth commit) and
/// `NativeComposed` (two or more mined backends that independently agreed). A task
/// with neither is `None`, which drives `aoa_gap::compute_gap` to `Unavailable`
/// (gap:unavailable); `SynthesizedFromVisible` is never produced here because
/// codeprobe never derives the held-out answer from the visible spec.
pub fn classify_provenance(task: &CodeprobeTask) -> HeldOutProvenance {
    let externally_composed = task.ground_truth_commit.is_some() && !task.oracle_files.is_empty();
    if externally_composed {
        return HeldOutProvenance::External;
    }

    let distinct_backends: BTreeSet<&BTreeSet<String>> = task.accepted_solutions.iter().collect();
    if distinct_backends.len() >= 2 {
        return HeldOutProvenance::NativeComposed;
    }

    HeldOutProvenance::None
}
