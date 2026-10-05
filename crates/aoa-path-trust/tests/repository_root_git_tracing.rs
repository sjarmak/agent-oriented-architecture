#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use aoa_path_trust::resolve_repository_root;

const RESOLVING_CHILD_REPOSITORY: &str = "AOA_TEST_RESOLVES_THE_ROOT_OF";

const RESOLVING_CHILD_VARIABLE: &str = "AOA_TEST_RESOLVES_THE_ROOT_UNDER";

const THIS_TEST: &str = "resolves_the_same_root_however_the_caller_traces_git";

const STANDARD_OUTPUT: &str = "/dev/stdout";

const TRACE2_TO_STANDARD_OUTPUT_CONFIG: &str = "[trace2]\n\tnormalTarget = /dev/stdout\n\teventTarget = /dev/stdout\n\tperfTarget = /dev/stdout\n";

fn resolve_as_the_child(repo: &Path) {
    let variable = std::env::var_os(RESOLVING_CHILD_VARIABLE)
        .expect("the parent names the variable the child inherits");
    assert!(
        std::env::var_os(&variable).is_some(),
        "the child did not inherit {variable:?}"
    );
    assert_eq!(resolve_repository_root(repo).unwrap(), repo);
}

fn child_report_when_the_root_changes(
    repo: &Path,
    variable: &str,
    value: &OsStr,
) -> Option<String> {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", THIS_TEST, "--nocapture"])
        .env(RESOLVING_CHILD_REPOSITORY, repo)
        .env(RESOLVING_CHILD_VARIABLE, variable)
        .env(variable, value)
        .output()
        .unwrap();
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() && report.contains("1 passed") {
        None
    } else {
        Some(report)
    }
}

#[test]
fn resolves_the_same_root_however_the_caller_traces_git() {
    if let Some(repo) = std::env::var_os(RESOLVING_CHILD_REPOSITORY) {
        resolve_as_the_child(&PathBuf::from(repo));
        return;
    }

    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path().canonicalize().unwrap().join("repo");
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .arg(&repo)
        .status()
        .expect("git is available for repository-boundary tests");
    assert!(initialized.success(), "git init failed for test fixture");
    let tracing_config = fixture.path().join("tracing.gitconfig");
    std::fs::write(&tracing_config, TRACE2_TO_STANDARD_OUTPUT_CONFIG).unwrap();
    assert_eq!(resolve_repository_root(&repo).unwrap(), repo);

    let standard_output = OsString::from(STANDARD_OUTPUT);
    let tracing_config = tracing_config.into_os_string();
    let cases = [
        ("GIT_TRACE", &standard_output),
        ("GIT_TRACE_PERFORMANCE", &standard_output),
        ("GIT_TRACE2", &standard_output),
        ("GIT_TRACE2_EVENT", &standard_output),
        ("GIT_TRACE2_PERF", &standard_output),
        ("GIT_CONFIG_GLOBAL", &tracing_config),
        ("GIT_CONFIG_SYSTEM", &tracing_config),
    ];
    let changed_the_answer: Vec<(&str, String)> = cases
        .into_iter()
        .filter_map(|(variable, value)| {
            child_report_when_the_root_changes(&repo, variable, value)
                .map(|report| (variable, report))
        })
        .collect();

    assert!(
        changed_the_answer.is_empty(),
        "the caller's git tracing changed the resolved root: {changed_the_answer:#?}"
    );
}
