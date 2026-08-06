use aoa_domain::{ExposureStatus, HeldOutProvenance};
use aoa_trace::Confidence;

use aoa_falsify::{
    falsify, is_eligible, ConventionInputs, Eligibility, FalsifyConfig, FalsifyError, FalsifyInput,
    PairTask, RepoResult, RepoRun, ScoringConvention, Verdict,
};

/// An eligible repo: high-confidence, native-composed, calibrated.
fn eligible() -> Eligibility {
    Eligibility {
        confidence: Confidence::High,
        native_span: HeldOutProvenance::NativeComposed,
        calibrated: true,
        exposure: ExposureStatus::Unexposed,
    }
}

#[test]
fn exposed_repo_is_not_eligible_to_vote() {
    let eligibility = Eligibility {
        exposure: ExposureStatus::Exposed,
        ..eligible()
    };

    assert!(!is_eligible(&eligibility));
}

/// One identical-pair task with the given two held-out outcomes, default scoring
/// inputs (mid locality, depth 1) that every default convention admits.
fn pair(task_id: u64, repo_ok: bool, harness_ok: bool) -> PairTask {
    PairTask {
        task_id: format!("task-{task_id}"),
        repo_observation_id: format!("repo-observation-{task_id}"),
        harness_observation_id: format!("harness-observation-{task_id}"),
        is_identical_pair: true,
        repo_held_out_success: repo_ok,
        harness_held_out_success: harness_ok,
        convention_inputs: ConventionInputs::Edit {
            edit_locality: 0.5,
            mutation_depth: 1,
        },
    }
}

/// The answer-task analogue of [`pair`]: focused traces (locality 1.0) at depth
/// 0 in both arms, admitted by every pre-registered answer convention.
fn answer_pair(task_id: u64, repo_ok: bool, harness_ok: bool) -> PairTask {
    PairTask {
        task_id: format!("task-{task_id}"),
        repo_observation_id: format!("repo-observation-{task_id}"),
        harness_observation_id: format!("harness-observation-{task_id}"),
        is_identical_pair: true,
        repo_held_out_success: repo_ok,
        harness_held_out_success: harness_ok,
        convention_inputs: ConventionInputs::Answer {
            repo_trace_locality: 1.0,
            harness_trace_locality: 1.0,
            repo_trace_reach_depth: 0,
            harness_trace_reach_depth: 0,
        },
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

#[test]
fn external_held_out_provenance_is_eligible() {
    assert!(is_eligible(&Eligibility {
        confidence: Confidence::High,
        native_span: HeldOutProvenance::External,
        calibrated: true,
        exposure: ExposureStatus::Unexposed,
    }));
}

#[test]
fn all_external_manifest_votes_in_r0() {
    let repos = (0..5)
        .map(|i| RepoResult {
            repo_id: format!("external-{i}"),
            eligibility: Eligibility {
                confidence: Confidence::High,
                native_span: HeldOutProvenance::External,
                calibrated: true,
                exposure: ExposureStatus::Unexposed,
            },
            runs: stable_runs(3, vec![pair(1, true, false)]),
            holdout_size: 40,
        })
        .collect();

    let report = falsify(&input(repos)).expect("external manifest is valid");

    assert_eq!(report.verdict, Verdict::Proceed);
    assert_eq!(report.eligible_repos.len(), 5);
    assert!(report.excluded_repos.is_empty());
}

#[test]
fn falsify_input_rejects_unknown_fields_at_owned_boundaries() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/proceed_input.json"))
            .expect("fixture parses as JSON");
    let boundaries = [
        ("input", ""),
        ("repo result", "/repos/0"),
        ("eligibility", "/repos/0/eligibility"),
        ("repo run", "/repos/0/runs/0"),
        ("pair task", "/repos/0/runs/0/tasks/0"),
        (
            "convention inputs",
            "/repos/0/runs/0/tasks/0/convention_inputs",
        ),
        ("config", "/config"),
        ("scoring convention", "/config/conventions/0"),
    ];

    for (name, pointer) in boundaries {
        let mut candidate = fixture.clone();
        candidate
            .pointer_mut(pointer)
            .and_then(serde_json::Value::as_object_mut)
            .unwrap_or_else(|| panic!("{name} boundary is an object"))
            .insert("unexpected".to_string(), serde_json::Value::Bool(true));

        let error = serde_json::from_value::<FalsifyInput>(candidate)
            .expect_err(&format!("{name} accepted an unknown field"));
        assert!(
            error.to_string().contains("unknown field `unexpected`"),
            "{name}: {error}"
        );
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
            runs: stable_runs(3, vec![pair(1, true, false), non_paired.clone()]),
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
        .iter()
        .any(|c| c.name == "alternative_metric_weights"));
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
            exposure: ExposureStatus::Unexposed,
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
            exposure: ExposureStatus::Unexposed,
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
            exposure: ExposureStatus::Unexposed,
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

// --- answer-task convention family (pre-registered 2026-07-04, aoa-dhk.1) ----

/// A repo of answer-shaped pairs, stable across `k_runs` and eligible.
fn answer_repo(id: &str, tasks: Vec<PairTask>) -> RepoResult {
    RepoResult {
        repo_id: id.to_string(),
        eligibility: eligible(),
        runs: stable_runs(3, tasks),
        holdout_size: 40,
    }
}

fn answer_input(repos: Vec<RepoResult>) -> FalsifyInput {
    FalsifyInput {
        repos,
        config: FalsifyConfig {
            conventions: ScoringConvention::admissible_answer(),
            ..FalsifyConfig::default()
        },
    }
}

/// Answer-shaped inputs score end to end under the answer convention set, and
/// the report names the family and the trace-convention names as data.
#[test]
fn answer_family_scores_and_reports_family_as_data() {
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| answer_repo(&format!("r{i}"), vec![answer_pair(1, true, false)]))
        .collect();

    let report = falsify(&answer_input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Proceed);

    let value: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(value["convention_family"], "answer");
    assert!(report
        .conventions_tried
        .iter()
        .any(|c| c.name == "trace_locality_floor"));
    assert!(report
        .conventions_tried
        .iter()
        .any(|c| c.name == "trace_reach_depth_k"));
}

/// falsification.json emits every convention's FULL parameters, not just names:
/// a reader can verify the thresholds, depths, and weights actually applied.
#[test]
fn report_emits_full_convention_parameters() {
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| answer_repo(&format!("r{i}"), vec![answer_pair(1, true, false)]))
        .collect();

    let report = falsify(&answer_input(repos)).expect("falsify runs");
    let value: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    let conventions = value["conventions_tried"].as_array().expect("array");
    assert_eq!(conventions.len(), 4);

    let depth_k = conventions
        .iter()
        .find(|c| c["name"] == "trace_reach_depth_k")
        .expect("depth-k present");
    assert_eq!(depth_k["max_depth"], 3);
    assert_eq!(depth_k["locality_threshold"], 0.0);

    let weights = conventions
        .iter()
        .find(|c| c["name"] == "alternative_metric_weights")
        .expect("weights present");
    assert_eq!(weights["repo_weight"], 0.75);
    assert_eq!(weights["harness_weight"], 1.25);
}

/// A hand-edited convention set — tampered threshold/depth behind unchanged
/// names, or a dropped entry — is a structural error. The pre-registered
/// admissible set is the only set the gate accepts; there is no override.
#[test]
fn tampered_convention_set_is_a_structural_error() {
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| answer_repo(&format!("r{i}"), vec![answer_pair(1, true, false)]))
        .collect();

    // The demonstrated tamper: strengthen the floor and zero the depth bound.
    let mut tampered = ScoringConvention::admissible_answer();
    tampered[0].locality_threshold = 0.9;
    tampered[2].max_depth = 0;
    let err = falsify(&FalsifyInput {
        repos: repos.clone(),
        config: FalsifyConfig {
            conventions: tampered,
            ..FalsifyConfig::default()
        },
    })
    .unwrap_err();
    assert!(matches!(
        err,
        FalsifyError::ConventionSetNotPreRegistered { .. }
    ));

    // A subset (dropped convention) is equally inadmissible.
    let mut subset = ScoringConvention::admissible_answer();
    subset.pop();
    let err = falsify(&FalsifyInput {
        repos,
        config: FalsifyConfig {
            conventions: subset,
            ..FalsifyConfig::default()
        },
    })
    .unwrap_err();
    assert!(matches!(
        err,
        FalsifyError::ConventionSetNotPreRegistered { .. }
    ));
}

/// TOTAL exclusion is a failed precondition, not vacuous invariance: when a
/// configured convention admits ZERO pairs (every repo's harness trace-reach
/// saturates beyond depth-k), the gate must abstain naming the convention.
/// Regression: the zero-admission deltas used to compare `0.0 >= 0.0`, so every
/// repo "voted proceed" under the all-excluding convention and the invariance
/// check passed on no evidence.
#[test]
fn answer_total_exclusion_under_depth_k_is_inconclusive_not_proceed() {
    let saturated = PairTask {
        convention_inputs: ConventionInputs::Answer {
            repo_trace_locality: 1.0,
            harness_trace_locality: 1.0,
            repo_trace_reach_depth: 0,
            harness_trace_reach_depth: aoa_falsify::UNREACHABLE_TRACE_REACH_DEPTH,
        },
        ..answer_pair(1, true, false)
    };
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| answer_repo(&format!("r{i}"), vec![saturated.clone()]))
        .collect();

    let report = falsify(&answer_input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("trace_reach_depth_k") && n.contains("zero")),
        "abstention must name the all-excluding convention, got {:?}",
        report.notes
    );
}

/// The edit family closes the same hole: pairs all deeper than the
/// mutation-surface depth-k bound leave that convention with zero admissions,
/// and the verdict abstains instead of proceeding.
#[test]
fn edit_total_exclusion_under_depth_k_is_inconclusive_not_proceed() {
    let deep = PairTask {
        convention_inputs: ConventionInputs::Edit {
            edit_locality: 0.5,
            mutation_depth: 10,
        },
        ..pair(1, true, false)
    };
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| RepoResult {
            repo_id: format!("r{i}"),
            eligibility: eligible(),
            runs: stable_runs(3, vec![deep.clone()]),
            holdout_size: 40,
        })
        .collect();

    let report = falsify(&input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("mutation_surface_depth_k") && n.contains("zero")),
        "abstention must name the all-excluding convention, got {:?}",
        report.notes
    );
}

/// A repo whose runs carry no identical-pair tasks casts no vote even under the
/// canonical convention: the base tally abstains rather than counting the empty
/// repo as a proceed vote.
#[test]
fn repo_with_zero_identical_pairs_cannot_vote() {
    let mut non_paired = pair(1, true, false);
    non_paired.is_identical_pair = false;

    let mut repos: Vec<RepoResult> = (0..4)
        .map(|i| repo(&format!("r{i}"), true, false))
        .collect();
    repos.push(RepoResult {
        repo_id: "empty".to_string(),
        eligibility: eligible(),
        runs: stable_runs(3, vec![non_paired]),
        holdout_size: 40,
    });

    let report = falsify(&input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("empty") && n.contains("canonical")),
        "abstention must name the evidence-free repo, got {:?}",
        report.notes
    );
}

/// A deep (or unreachable) trace-reach in EITHER arm drops the task under the
/// depth-k convention; a proceed that depends on such tasks flips and the
/// verdict downgrades to inconclusive.
#[test]
fn answer_depth_flip_downgrades_to_inconclusive() {
    // Per repo: a repo-arm win whose harness-arm trace never reaches the oracle
    // chain (unreachable depth), plus a shallow harness-arm win. Canonical: 0.5
    // vs 0.5, tie => every repo votes proceed. Under trace_reach_depth_k only
    // the shallow harness win is admitted => the vote flips => inconclusive.
    let deep_repo_win = PairTask {
        convention_inputs: ConventionInputs::Answer {
            repo_trace_locality: 1.0,
            harness_trace_locality: 1.0,
            repo_trace_reach_depth: 0,
            harness_trace_reach_depth: aoa_falsify::UNREACHABLE_TRACE_REACH_DEPTH,
        },
        ..answer_pair(1, true, false)
    };
    let shallow_harness_win = answer_pair(2, false, true);
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| {
            answer_repo(
                &format!("r{i}"),
                vec![deep_repo_win.clone(), shallow_harness_win.clone()],
            )
        })
        .collect();

    let report = falsify(&answer_input(repos)).expect("falsify runs");
    assert_eq!(report.verdict, Verdict::Inconclusive);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("trace_reach_depth_k")));
}

/// Tasks carrying convention inputs of both families in one input are a
/// structural error — never scored under a single convention set.
#[test]
fn mixed_input_families_are_an_error() {
    let mut repos: Vec<RepoResult> = (0..4)
        .map(|i| repo(&format!("r{i}"), true, false))
        .collect();
    repos.push(answer_repo("r4", vec![answer_pair(1, true, false)]));

    let err = falsify(&input(repos)).unwrap_err();
    assert_eq!(err, FalsifyError::MixedInputFamilies);
}

/// A convention set whose family does not match the tasks' inputs is a
/// structural error — otherwise every convention would silently admit nothing.
#[test]
fn convention_family_mismatch_is_an_error() {
    // Edit-family default conventions over answer-shaped tasks.
    let repos: Vec<RepoResult> = (0..5)
        .map(|i| answer_repo(&format!("r{i}"), vec![answer_pair(1, true, false)]))
        .collect();

    let err = falsify(&input(repos)).unwrap_err();
    assert!(matches!(err, FalsifyError::ConventionFamilyMismatch { .. }));
}
