//! Integration tests for the codeprobe-mined task loader.
//!
//! Fixtures under `tests/fixtures/` are committed sanitized codeprobe tasks, so
//! these tests run without the codeprobe project present.

use std::path::PathBuf;

use aoa_bench::{load_task, BenchError, CodeprobeTask};
use aoa_gap::{compute_gap, GapOutcome, HeldOutProvenance};
use aoa_metrics::MetricError;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load(name: &str) -> CodeprobeTask {
    load_task(fixture(name)).expect("fixture loads")
}

// --- AC1: a loader reads a codeprobe task dir into AOA task inputs ---

#[test]
fn loads_org_scale_metadata_task_into_aoa_inputs() {
    let task = load("external-filelist-000");
    assert_eq!(task.id, "external-filelist-000");
    assert_eq!(task.repo, "sample/widget");
    assert!(task.instruction.contains("Widget"));
    assert!(task.oracle_files.contains("src/widget/core.py"));
    assert!(task.oracle_files.contains("tests/test_core.py"));
    assert_eq!(
        task.ground_truth_commit.as_deref(),
        Some("a3c0ffee1234567890abcdef1234567890abcdef")
    );
}

#[test]
fn loads_probe_toml_task_into_aoa_inputs() {
    let task = load("native-consensus-001");
    assert_eq!(task.id, "native-consensus-001");
    assert_eq!(task.repo, "sample/widget");
    assert!(task.instruction.contains("parse_config"));
    assert!(task.oracle_files.contains("src/widget/config.py"));
}

#[test]
fn loads_root_level_ground_truth_when_no_tests_dir() {
    // Org-scale tasks can carry ground_truth.json at the task root rather than
    // under tests/, and with no inline oracle_answer in the manifest. This is the
    // only layout that exercises the ground_truth_dir() task-root fallback to feed
    // oracle_files — every other fixture either has an inline oracle or a
    // tests/ground_truth.json, so the fallback path was previously untested.
    let task = load("root-gt-004");
    assert_eq!(task.id, "root-gt-004");
    assert_eq!(task.repo, "sample/widget");
    // oracle_files must come from the root-level ground_truth.json `expected` set.
    assert_eq!(
        task.oracle_files.iter().cloned().collect::<Vec<_>>(),
        vec![
            "src/widget/cache.py".to_string(),
            "src/widget/store.py".to_string(),
        ],
        "oracle_files must be read from the root-level ground_truth.json"
    );
}

#[test]
fn missing_manifest_is_rejected_loudly() {
    let err = load_task(fixture("does-not-exist")).unwrap_err();
    assert!(matches!(err, BenchError::NotATask(_)));
}

// --- AC2: held-out provenance surfaced as External / NativeComposed, never
//          SynthesizedFromVisible; no independent leg -> None -> gap:unavailable ---

#[test]
fn file_list_oracle_with_commit_is_external() {
    let task = load("external-filelist-000");
    assert_eq!(task.held_out_provenance(), HeldOutProvenance::External);
}

#[test]
fn probe_layout_commit_in_ground_truth_is_external() {
    // A task.toml task with no org-scale metadata block, whose mining commit
    // lives in ground_truth.json, is still recognized as externally composed.
    let task = load("external-toml-003");
    assert_eq!(
        task.ground_truth_commit.as_deref(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(task.held_out_provenance(), HeldOutProvenance::External);
}

#[test]
fn two_agreeing_backends_are_native_composed() {
    let task = load("native-consensus-001");
    assert_eq!(
        task.held_out_provenance(),
        HeldOutProvenance::NativeComposed
    );
}

#[test]
fn native_composed_backends_are_auditable_from_the_loaded_task() {
    // AC2: the NativeComposed decision must be auditable from the loaded
    // structure — the backend identities that agreed are read straight from
    // divergence_report.json, not inferred from file-set cardinality.
    let task = load("native-consensus-001");
    let backends: Vec<&str> = task
        .accepted_solutions
        .iter()
        .map(|s| s.backend.as_str())
        .collect();
    assert_eq!(backends, vec!["ast", "treesitter"]);
}

#[test]
fn provenance_is_never_synthesized_from_visible() {
    for name in [
        "external-filelist-000",
        "external-toml-003",
        "native-consensus-001",
        "no-heldout-002",
    ] {
        let task = load(name);
        assert_ne!(
            task.held_out_provenance(),
            HeldOutProvenance::SynthesizedFromVisible,
            "{name} must never be synthesized-from-visible"
        );
    }
}

#[test]
fn no_independent_held_out_leg_is_none_and_gap_unavailable() {
    let task = load("no-heldout-002");
    assert_eq!(task.held_out_provenance(), HeldOutProvenance::None);

    // The run carries provenance None, so the gap gate refuses to compute a gap.
    let run = task.to_run_result(true, false);
    let outcome = compute_gap(&run).expect("None provenance is not an error");
    assert_eq!(outcome, GapOutcome::Unavailable);
}

#[test]
fn external_task_yields_a_computable_gap() {
    let task = load("external-filelist-000");
    let run = task.to_run_result(true, false);
    let outcome = compute_gap(&run).expect("external provenance computes a gap");
    assert!(matches!(outcome, GapOutcome::Available { .. }));
}

// --- AC3: oracle provides G_t and >=2 accepted-solutions for edit-locality;
//          <2 -> InsufficientAcceptedSolutions, never fabricated ---

#[test]
fn gold_set_is_the_oracle_file_set() {
    let task = load("external-filelist-000");
    assert_eq!(task.gold_set(), &task.oracle_files);
    assert!(!task.gold_set().is_empty());
}

#[test]
fn two_accepted_solutions_supply_edit_locality_anchors() {
    let task = load("native-consensus-001");
    let anchors = task
        .edit_locality_anchors()
        .expect("two distinct accepted solutions are available");
    assert!(anchors.accepted_solutions.len() >= 2);
    assert!(anchors.gold_set.contains("src/widget/config.py"));
}

#[test]
fn fewer_than_two_accepted_solutions_surfaces_insufficient_never_fabricated() {
    let task = load("no-heldout-002");
    assert!(task.accepted_solutions.len() < 2);
    let err = task.edit_locality_anchors().unwrap_err();
    match err {
        MetricError::InsufficientAcceptedSolutions(n) => {
            assert_eq!(n, task.accepted_solutions.len())
        }
    }
}
