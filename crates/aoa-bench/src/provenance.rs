use std::collections::BTreeSet;

use aoa_gap::HeldOutProvenance;

use crate::task::CodeprobeTask;

/// Classify a task's held-out provenance from its mined structure.
///
/// codeprobe mines from repo history, so the only legitimate labels are
/// `External` (a `file_list` oracle anchored to a real ground-truth commit) and
/// `NativeComposed` (two or more independently-mined backends agreed in consensus
/// mining). The `NativeComposed` decision is by backend *identity*, not file-set
/// cardinality: two backends that mined the same files are two independent runs,
/// the strongest agreement signal — not a single collapsed solution. A task with
/// neither leg is `None`, which drives `aoa_gap::compute_gap` to `Unavailable`
/// (gap:unavailable); `SynthesizedFromVisible` is never produced here because
/// codeprobe never derives the held-out answer from the visible spec.
pub(crate) fn classify_provenance(task: &CodeprobeTask) -> HeldOutProvenance {
    let externally_composed = task.ground_truth_commit.is_some() && !task.oracle_files.is_empty();
    if externally_composed {
        return HeldOutProvenance::External;
    }

    let distinct_backends: BTreeSet<&str> = task
        .accepted_solutions
        .iter()
        .map(|s| s.backend.as_str())
        .collect();
    if distinct_backends.len() >= 2 {
        return HeldOutProvenance::NativeComposed;
    }

    HeldOutProvenance::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::AcceptedSolution;

    fn solution(backend: &str, files: &[&str]) -> AcceptedSolution {
        AcceptedSolution {
            backend: backend.to_string(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    fn task(
        commit: Option<&str>,
        oracle: &[&str],
        accepted: Vec<AcceptedSolution>,
    ) -> CodeprobeTask {
        CodeprobeTask {
            id: "t".into(),
            repo: "r".into(),
            instruction: "i".into(),
            oracle_files: oracle.iter().map(|f| f.to_string()).collect(),
            ground_truth_commit: commit.map(str::to_string),
            accepted_solutions: accepted,
        }
    }

    #[test]
    fn two_backends_with_identical_files_are_native_composed() {
        // The bug this bead fixes: identical file-sets from two distinct backends
        // are the STRONGEST agreement, and must classify NativeComposed — not
        // collapse to None as file-set-equality dedup did.
        let t = task(
            None,
            &["a.py"],
            vec![
                solution("ast", &["a.py", "b.py"]),
                solution("treesitter", &["a.py", "b.py"]),
            ],
        );
        assert_eq!(t.held_out_provenance(), HeldOutProvenance::NativeComposed);
    }

    #[test]
    fn two_backends_with_different_files_are_native_composed() {
        let t = task(
            None,
            &["a.py"],
            vec![
                solution("ast", &["a.py"]),
                solution("treesitter", &["a.py", "b.py"]),
            ],
        );
        assert_eq!(t.held_out_provenance(), HeldOutProvenance::NativeComposed);
    }

    #[test]
    fn a_single_backend_is_not_native_composed() {
        let t = task(None, &["a.py"], vec![solution("ast", &["a.py"])]);
        assert_eq!(t.held_out_provenance(), HeldOutProvenance::None);
    }

    #[test]
    fn a_real_commit_makes_it_external_before_backend_count() {
        // External short-circuits: a task with both a commit and backends is
        // External (the commit is the stronger contamination-free anchor).
        let t = task(
            Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"),
            &["a.py"],
            vec![
                solution("ast", &["a.py"]),
                solution("treesitter", &["a.py"]),
            ],
        );
        assert_eq!(t.held_out_provenance(), HeldOutProvenance::External);
    }
}
