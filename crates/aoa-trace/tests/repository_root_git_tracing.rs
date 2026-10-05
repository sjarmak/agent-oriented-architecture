#![cfg(unix)]

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use aoa_trace::resolve_repository_root;

const STANDARD_OUTPUT: &str = "/dev/stdout";

const TRACE2_TO_STANDARD_OUTPUT_CONFIG: &str = "[trace2]\n\tnormalTarget = /dev/stdout\n\teventTarget = /dev/stdout\n\tperfTarget = /dev/stdout\n";

fn resolves_to(repo: &Path, variable: &str, value: &OsString) -> bool {
    std::env::set_var(variable, value);
    let resolved = resolve_repository_root(repo);
    std::env::remove_var(variable);
    resolved.ok().as_deref() == Some(repo)
}

#[test]
fn resolves_the_same_root_however_the_caller_traces_git() {
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
    let changed_the_answer: Vec<&str> = cases
        .into_iter()
        .filter(|(variable, value)| !resolves_to(&repo, variable, value))
        .map(|(variable, _)| variable)
        .collect();

    assert!(
        changed_the_answer.is_empty(),
        "the caller's git tracing changed the resolved root: {changed_the_answer:?}"
    );
}
