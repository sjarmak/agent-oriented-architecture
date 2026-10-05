use std::ffi::OsStr;
use std::process::Command;

use aoa_trace::git_free_of_inherited_state;

const INSPECTING_CHILD: &str = "AOA_TEST_INSPECTS_INHERITED_GIT_ENVIRONMENT";

const THIS_TEST: &str = "removes_inherited_redirect_and_trace_variables_from_the_git_command";

const INHERITED_AND_REMOVED: [&str; 4] = [
    "GIT_REDIRECT_STDOUT",
    "GIT_REDIRECT_STDERR",
    "GIT_REDIRECT_STDIN",
    "GIT_TRACE_PERFORMANCE",
];

const INHERITED_VALUE: &str = "inherited-by-the-test";

fn not_removed_by(command: &Command) -> Vec<&'static str> {
    INHERITED_AND_REMOVED
        .into_iter()
        .filter(|variable| {
            !command
                .get_envs()
                .any(|(name, value)| name == OsStr::new(variable) && value.is_none())
        })
        .collect()
}

#[test]
fn removes_inherited_redirect_and_trace_variables_from_the_git_command() {
    if std::env::var_os(INSPECTING_CHILD).is_some() {
        let inherited: Vec<&str> = INHERITED_AND_REMOVED
            .into_iter()
            .filter(|variable| std::env::var_os(variable).is_some())
            .collect();
        assert_eq!(
            inherited, INHERITED_AND_REMOVED,
            "the child did not inherit every variable under test"
        );
        let command = git_free_of_inherited_state();
        let kept = not_removed_by(&command);
        assert!(
            kept.is_empty(),
            "git would still inherit from the caller: {kept:?}"
        );
        return;
    }

    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", THIS_TEST, "--nocapture"])
        .env(INSPECTING_CHILD, "1");
    for variable in INHERITED_AND_REMOVED {
        child.env(variable, INHERITED_VALUE);
    }
    let output = child.output().unwrap();
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{report}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        report.contains("1 passed"),
        "the child ran no inspection: {report}"
    );
}
