use super::behavioral_signal::seed_live_sessions;
use super::eval_run_graph_coverage::repo_with;
use super::*;

const READINESS_COMMANDS: [&str; 3] = ["audit", "recommend", "report"];

fn readiness_json(command: &str, repo: &Path) -> Value {
    let output = aoa()
        .args([command, "--json", "--repo"])
        .arg(repo)
        .output()
        .expect("run");
    assert!(output.status.success(), "{command} failed: {output:?}");
    serde_json::from_slice(&output.stdout).expect("valid json")
}

fn exclusion_reasons(audit: &Value) -> Vec<&str> {
    audit["live_observations"]
        .as_array()
        .expect("live observations")
        .iter()
        .map(|observation| {
            assert_eq!(observation["state"]["status"], "excluded");
            observation["state"]["reason"].as_str().expect("reason")
        })
        .collect()
}

#[test]
fn readiness_commands_report_a_graph_that_missed_the_main_language() {
    let repo = repo_with(100, 1);

    for command in READINESS_COMMANDS {
        let parsed = readiness_json(command, repo.path());

        assert_eq!(
            parsed["graph_coverage"]["indexed"]["Python"], 1,
            "{command}"
        );
        assert_eq!(
            parsed["graph_coverage"]["unindexed"]["TypeScript"], 100,
            "{command}"
        );
        let reason = parsed["graph_degrade_reason"]
            .as_str()
            .unwrap_or_else(|| panic!("{command} states no degrade reason"));
        assert!(
            reason.contains("TypeScript (100 files)"),
            "{command}: {reason}"
        );
        assert!(reason.contains("live sessions"), "{command}: {reason}");
        assert!(
            !reason.contains("--scip-index"),
            "{command} has no such flag to recommend: {reason}"
        );
    }
}

#[test]
fn readiness_commands_name_the_unindexed_language_when_no_file_was_indexable() {
    let repo = repo_with(5, 0);

    for command in READINESS_COMMANDS {
        let parsed = readiness_json(command, repo.path());

        assert_eq!(
            parsed["graph_coverage"]["indexed"],
            serde_json::json!({}),
            "{command}"
        );
        let reason = parsed["graph_degrade_reason"].as_str().expect("reason");
        assert!(reason.contains("0 of 5 source files indexed"), "{reason}");
        assert!(reason.contains("TypeScript (5 files)"), "{reason}");
    }
}

#[test]
fn readiness_commands_keep_a_mixed_repo_graph_and_report_what_it_skipped() {
    let repo = repo_with(3, 2);

    for command in READINESS_COMMANDS {
        let parsed = readiness_json(command, repo.path());

        assert_eq!(
            parsed["graph_coverage"]["indexed"]["Python"], 2,
            "{command}"
        );
        assert_eq!(
            parsed["graph_coverage"]["unindexed"]["TypeScript"], 3,
            "{command}"
        );
        assert!(parsed.get("graph_degrade_reason").is_none(), "{command}");
    }
}

#[test]
fn readiness_commands_report_full_coverage_for_a_python_only_repo() {
    let repo = repo_with(0, 2);

    for command in READINESS_COMMANDS {
        let parsed = readiness_json(command, repo.path());

        assert_eq!(
            parsed["graph_coverage"]["indexed"]["Python"], 2,
            "{command}"
        );
        assert_eq!(
            parsed["graph_coverage"]["unindexed"],
            serde_json::json!({}),
            "{command}"
        );
        assert!(parsed.get("graph_degrade_reason").is_none(), "{command}");
    }
}

#[test]
fn live_sessions_are_not_scored_against_a_graph_that_missed_the_main_language() {
    let degraded = repo_with(100, 1);
    seed_live_sessions(degraded.path(), 3);
    let mixed = repo_with(3, 2);
    seed_live_sessions(mixed.path(), 3);

    let from_audit = readiness_json("audit", degraded.path());
    assert_eq!(exclusion_reasons(&from_audit), ["symbol_graph_missing"; 3]);
    let from_report = readiness_json("report", degraded.path());
    assert_eq!(
        exclusion_reasons(&from_report["audit"]),
        ["symbol_graph_missing"; 3]
    );

    let weighted = readiness_json("audit", mixed.path());
    assert_eq!(exclusion_reasons(&weighted), ["task_context_missing"; 3]);
}

#[test]
fn readiness_commands_name_the_unindexed_language_in_the_human_register() {
    let repo = repo_with(100, 1);

    for command in READINESS_COMMANDS {
        aoa()
            .args([command, "--repo"])
            .arg(repo.path())
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "graph coverage: Python 1 of 101 source files indexed; \
                 not indexed: TypeScript (100 files)",
            ))
            .stdout(predicate::str::contains(
                "warning: best-effort graph indexes Python only",
            ));
    }
}

#[test]
fn readiness_commands_print_coverage_without_a_warning_for_a_mixed_repo() {
    let repo = repo_with(3, 2);

    for command in READINESS_COMMANDS {
        aoa()
            .args([command, "--repo"])
            .arg(repo.path())
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "graph coverage: Python 2 of 5 source files indexed; \
                 not indexed: TypeScript (3 files)",
            ))
            .stdout(predicate::str::contains("best-effort graph indexes").not());
    }
}

#[test]
fn report_states_graph_coverage_once_and_agrees_with_audit_and_recommend() {
    let repo = repo_with(100, 1);
    seed_live_sessions(repo.path(), 3);

    let report = readiness_json("report", repo.path());
    assert!(report["audit"].get("graph_coverage").is_none());
    assert!(report["recommendations"].get("graph_coverage").is_none());

    for (command, section) in [("audit", "audit"), ("recommend", "recommendations")] {
        let mut standalone = readiness_json(command, repo.path());
        let fields = standalone.as_object_mut().expect("object");
        assert_eq!(
            fields.remove("graph_coverage").as_ref(),
            Some(&report["graph_coverage"]),
            "{command}"
        );
        assert_eq!(
            fields.remove("graph_degrade_reason").as_ref(),
            Some(&report["graph_degrade_reason"]),
            "{command}"
        );
        assert_eq!(standalone, report[section], "{command}");
    }
}
