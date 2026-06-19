use aoa_gap::HeldOutProvenance;
use aoa_metrics::Confidence;

use aoa_falsify::{
    falsify, Eligibility, FalsifyConfig, FalsifyError, FalsifyInput, PairTask, RepoResult, RepoRun,
    Verdict,
};

/// An eligible repo: high-confidence, native-composed, calibrated.
fn eligible() -> Eligibility {
    Eligibility {
        confidence: Confidence::High,
        native_span: HeldOutProvenance::NativeComposed,
        calibrated: true,
    }
}

/// One identical-pair task with the given two held-out outcomes, default scoring
/// inputs (mid locality, depth 1) that every default convention admits.
fn pair(task_id: u64, repo_ok: bool, harness_ok: bool) -> PairTask {
    PairTask {
        task_id,
        is_identical_pair: true,
        repo_held_out_success: repo_ok,
        harness_held_out_success: harness_ok,
        edit_locality: 0.5,
        mutation_depth: 1,
    }
}

/// Replicate one task list across `k` identical fixed-seed runs (stable).
fn stable_runs(k: u32, tasks: Vec<PairTask>) -> Vec<RepoRun> {
    (0..k)
        .map(|seed| RepoRun {
            seed: seed as u64,
            tasks: tasks.clone(),
        })
        .collect()
}

/// A repo whose single identical-pair task has the given two outcomes, stable
/// across `k_runs` and with an ample holdout.
fn repo(id: &str, repo_ok: bool, harness_ok: bool) -> RepoResult {
    RepoResult {
        repo_id: id.to_string(),
        eligibility: eligible(),
        runs: stable_runs(3, vec![pair(1, repo_ok, harness_ok)]),
        holdout_size: 40,
    }
}

fn input(repos: Vec<RepoResult>) -> FalsifyInput {
    FalsifyInput {
        repos,
        config: FalsifyConfig::default(),
    }
}

/// Criterion 1: emits falsification.json with repo_delta, harness_delta, verdict.
#[test]
fn emits_falsification_json_with_required_fields() {
    let raw = include_str!("../fixtures/proceed_input.json");
    let parsed: FalsifyInput = serde_json::from_str(raw).expect("fixture parses");

    let report = falsify(&parsed).expect("falsify runs");
    let json = report.to_json().expect("serializes");

    assert!(json.contains("\"repo_delta\""));
    assert!(json.contains("\"harness_delta\""));
    assert!(json.contains("\"verdict\""));

    let round: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(round["repo_delta"].is_number());
    assert!(round["harness_delta"].is_number());
    assert_eq!(round["verdict"], "proceed");
}

/// Criterion 2: majority rule, table-driven, INCLUDING the tie case.
#[test]
fn majority_rule_table_driven_including_tie() {
    struct Case {
        name: &'static str,
        // per-repo: (repo_ok, harness_ok). repo_ok>=harness_ok => votes proceed.
        repos: Vec<(bool, bool)>,
        expected: Verdict,
    }

    let cases = vec![
        Case {
            name: "strict majority for proceed (5/5)",
            repos: vec![(true, false); 5],
            expected: Verdict::Proceed,
        },
        Case {
            name: "strict majority for proceed (3/5)",
            repos: vec![
                (true, false),
                (true, false),
                (true, false),
                (false, true),
                (false, true),
            ],
            expected: Verdict::Proceed,
        },
        Case {
            name: "minority pivots",
            repos: vec![
                (true, false),
                (true, false),
                (false, true),
                (false, true),
                (false, true),
            ],
            expected: Verdict::Pivot,
        },
        Case {
            name: "exact tie defaults to pivot (3 for, 3 against)",
            repos: vec![
                (true, false),
                (true, false),
                (true, false),
                (false, true),
                (false, true),
                (false, true),
            ],
            expected: Verdict::Pivot,
        },
    ];

    for case in cases {
        let repos: Vec<RepoResult> = case
            .repos
            .iter()
            .enumerate()
            .map(|(i, (r, h))| repo(&format!("r{i}"), *r, *h))
            .collect();
        let report = falsify(&input(repos)).expect("falsify runs");
        assert_eq!(report.verdict, case.expected, "case: {}", case.name);
    }
}

/// Criterion 3: only identical-pair tasks contribute; non-paired excluded.
#[test]
fn only_identical_pair_tasks_contribute() {
    // Each repo: one paired task (repo wins) plus a non-paired task that, if it
    // counted, would flip the harness arm to win. It must be excluded.
    let mut non_paired = pair(2, false, true);
    non_paired.is_identical_pair = false;

    let repos: Vec<RepoResult> = (0..5)
        .map(|i| RepoResult {
            repo_id: format!("r{i}"),
            eligibility: eligible(),
            runs: stable_runs(3, vec![pair(1, true, false), non_paired]),
            holdout_size: 40,
        })
        .collect();

    let report = falsify(&input(repos)).expect("falsify runs");
    // Only the paired (repo-wins) task counts => repo_delta 1.0, harness 0.0.
    assert_eq!(report.repo_delta, 1.0);
    assert_eq!(report.harness_delta, 0.0);
    assert_eq!(report.verdict, Verdict::Proceed);
}

/// Criterion 4: determinism gate — unstable across K runs => inconclusive.
#[test]
fn determinism_gate_unstable_runs_inconclusive() {
    // A balanced eligible set (two repo-wins, two harness-wins) plus one repo
    // whose vote flips across seeds. Run 0 the flipper votes for proceed (3 vs 2
    // => proceed); run 1 it votes against (2 vs 3 => pivot). The aggregate verdict
    // is therefore unstable across the fixed-seed runs.
    let mut repos: Vec<RepoResult> = vec![
        repo("for0", true, false),
        repo("for1", true, false),
        repo("against0", false, true),
        repo("against1", false, true),
    ];

    let unstable = RepoResult {
        repo_id: "unstable".to_string(),
        eligibility: eligible(),
        runs: vec![
            RepoRun {
                seed: 0,
                tasks: vec![pair(1, true, false)],
            }, // repo wins
            RepoRun {
                seed: 1,
                tasks: vec![pair(1, false, true)],
            }, // harness wins
            RepoRun {
                seed: 2,
                tasks: vec![pair(1, true, false)],
            },
        ],
        holdout_size: 40,
    };
    repos.push(unstable);

    let report = falsify(&input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert_ne!(report.verdict, Verdict::Proceed);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("unstable") || n.contains("differs")));
}

/// Criterion 5: convention-invariance — a proceed that flips under an admissible
/// convention is downgraded to inconclusive.
#[test]
fn convention_invariance_flip_downgrades_to_inconclusive() {
    // Under the canonical convention (no exclusion, equal weights) the repo arm
    // wins. But each repo's paired task has high edit-locality (1.0), so the
    // edit-locality CEILING convention (admits locality <= ... but the default
    // ceiling threshold is 1.0 so still admitted) — instead use the weighting:
    // make the repo-vs-harness margin razor-thin so the alternative_metric_weights
    // convention (repo 0.75 vs harness 1.25) flips the vote.
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| RepoResult {
            repo_id: format!("r{i}"),
            eligibility: eligible(),
            // Both arms succeed on the single task: canonical => repo_delta 1.0 ==
            // harness_delta 1.0 => repo votes proceed (>=). Under alternative
            // weights => repo 0.75 < harness 1.25 => repo no longer votes proceed.
            runs: stable_runs(3, vec![pair(1, true, true)]),
            holdout_size: 40,
        })
        .collect();

    let report = falsify(&input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("flips") && n.contains("alternative_metric_weights")));
    // Conventions are emitted as data.
    assert!(report
        .conventions_tried
        .contains(&"alternative_metric_weights".to_string()));
}

/// Criterion 6: ineligible repos (low-confidence / reconstructed) do not vote.
#[test]
fn ineligible_repos_excluded_from_voting() {
    // Five eligible repos that PIVOT (harness wins), plus three ineligible repos
    // that, if they voted, would swing to proceed. They must be excluded, and the
    // eligible majority must still pivot.
    let mut repos: Vec<RepoResult> = (0..5)
        .map(|i| repo(&format!("elig{i}"), false, true))
        .collect();

    // low-confidence repo
    repos.push(RepoResult {
        repo_id: "low_conf".to_string(),
        eligibility: Eligibility {
            confidence: Confidence::Low,
            native_span: HeldOutProvenance::NativeComposed,
            calibrated: true,
        },
        runs: stable_runs(3, vec![pair(1, true, false)]),
        holdout_size: 40,
    });
    // reconstructed (not native-composed) repo
    repos.push(RepoResult {
        repo_id: "reconstructed".to_string(),
        eligibility: Eligibility {
            confidence: Confidence::High,
            native_span: HeldOutProvenance::SynthesizedFromVisible,
            calibrated: true,
        },
        runs: stable_runs(3, vec![pair(1, true, false)]),
        holdout_size: 40,
    });
    // uncalibrated repo
    repos.push(RepoResult {
        repo_id: "uncalibrated".to_string(),
        eligibility: Eligibility {
            confidence: Confidence::High,
            native_span: HeldOutProvenance::NativeComposed,
            calibrated: false,
        },
        runs: stable_runs(3, vec![pair(1, true, false)]),
        holdout_size: 40,
    });

    let report = falsify(&input(repos)).expect("falsify runs");

    assert!(report.excluded_repos.contains(&"low_conf".to_string()));
    assert!(report.excluded_repos.contains(&"reconstructed".to_string()));
    assert!(report.excluded_repos.contains(&"uncalibrated".to_string()));
    assert_eq!(report.eligible_repos.len(), 5);
    // The eligible majority pivots; the ineligible "proceed" votes were ignored.
    assert_eq!(report.verdict, Verdict::Pivot);
}

/// Criterion 7: power precondition — small holdout refuses a significant verdict.
#[test]
fn power_precondition_small_holdout_inconclusive() {
    // Five repos that would proceed, but each holdout (10) is below the default
    // minimum (20).
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| RepoResult {
            repo_id: format!("r{i}"),
            eligibility: eligible(),
            runs: stable_runs(3, vec![pair(1, true, false)]),
            holdout_size: 10,
        })
        .collect();

    let report = falsify(&input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("power precondition fails")));
}

/// Criterion 7b: effect-size precondition — below min effect size => inconclusive.
#[test]
fn power_precondition_effect_size_inconclusive() {
    // Repo arm wins (effect > 0) but require a higher minimum effect size.
    let cfg = FalsifyConfig {
        min_effect_size: 0.9,
        ..FalsifyConfig::default()
    };
    // Only three of five repos show any separation, so the mean effect magnitude
    // (3 * 1.0 + 2 * 0.0) / 5 = 0.6 is below the 0.9 threshold.
    let repos: Vec<RepoResult> = vec![
        repo("r0", true, false),
        repo("r1", true, false),
        repo("r2", true, false),
        repo("r3", false, false),
        repo("r4", false, false),
    ];
    let report = falsify(&FalsifyInput { repos, config: cfg }).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(report.notes.iter().any(|n| n.contains("effect size")));
}

/// Criterion 8: inconclusive is never silently converted to pivot; preserved
/// verbatim in the json.
#[test]
fn inconclusive_preserved_verbatim_in_json() {
    // Trigger inconclusive via the power gate, then assert the json field reads
    // exactly "inconclusive" — not "pivot".
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| RepoResult {
            repo_id: format!("r{i}"),
            eligibility: eligible(),
            runs: stable_runs(3, vec![pair(1, true, false)]),
            holdout_size: 5,
        })
        .collect();

    let report = falsify(&input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);

    let json = report.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["verdict"], "inconclusive");
    assert_ne!(value["verdict"], "pivot");
}

/// Structural guard: fewer than five repos is an error, not a verdict.
#[test]
fn too_few_repos_is_error() {
    let repos: Vec<RepoResult> = (0..4)
        .map(|i| repo(&format!("r{i}"), true, false))
        .collect();
    let err = falsify(&input(repos)).unwrap_err();
    assert_eq!(err, FalsifyError::TooFewRepos(4));
}
