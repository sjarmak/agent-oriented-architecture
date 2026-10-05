use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;

use super::falsify_policy::init_git_repo;
use super::*;

const INHERITED_REPOSITORY_VARIABLES: [&str; 17] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
];

const INHERITED_TRACE_VARIABLES: [&str; 5] = [
    "GIT_TRACE",
    "GIT_TRACE_PERFORMANCE",
    "GIT_TRACE2",
    "GIT_TRACE2_EVENT",
    "GIT_TRACE2_PERF",
];

const STANDARD_OUTPUT: &str = "/dev/stdout";

fn repository_with_a_pre_commit_hook() -> TempDir {
    let repo = TempDir::new().expect("tempdir");
    init_git_repo(repo.path());
    let hook = repo.path().join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().expect("hooks directory")).unwrap();
    std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo
}

fn reports_the_pre_commit_plane_missing(repo: &Path, inherited: Option<(&str, &OsStr)>) -> bool {
    let mut audit = aoa();
    audit.args(["audit", "--json", "--repo"]).arg(repo);
    if let Some((variable, value)) = inherited {
        audit.env(variable, value);
    }
    let output = audit.output().expect("run");
    assert!(output.status.success(), "{inherited:?}: {output:?}");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    parsed["items"]
        .as_array()
        .expect("items")
        .iter()
        .any(|item| item["plane"] == "pre-commit")
}

fn variables_that_change_the_answer<'a>(variables: &[&'a str], value: &OsStr) -> Vec<&'a str> {
    let repo = repository_with_a_pre_commit_hook();
    assert!(!reports_the_pre_commit_plane_missing(repo.path(), None));
    variables
        .iter()
        .copied()
        .filter(|variable| {
            reports_the_pre_commit_plane_missing(repo.path(), Some((variable, value)))
        })
        .collect()
}

#[test]
fn audit_reads_the_pre_commit_plane_the_same_under_every_inherited_repository_variable() {
    let ambient = TempDir::new().expect("tempdir");
    let elsewhere = ambient.path().join("elsewhere");

    let changed_the_answer =
        variables_that_change_the_answer(&INHERITED_REPOSITORY_VARIABLES, elsewhere.as_os_str());

    assert!(
        changed_the_answer.is_empty(),
        "the caller's environment changed what the audit reports: {changed_the_answer:?}"
    );
}

#[test]
fn audit_reads_the_pre_commit_plane_the_same_when_the_caller_traces_git_to_standard_output() {
    let changed_the_answer =
        variables_that_change_the_answer(&INHERITED_TRACE_VARIABLES, OsStr::new(STANDARD_OUTPUT));

    assert!(
        changed_the_answer.is_empty(),
        "the caller's git tracing changed what the audit reports: {changed_the_answer:?}"
    );
}
