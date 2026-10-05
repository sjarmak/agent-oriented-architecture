use std::ffi::OsStr;

use super::cli_git_environment::{inferred_owners, repository_owned_by, ALICE};
use super::*;

fn ownership_distrusted() -> (&'static str, &'static OsStr) {
    ("GIT_TEST_ASSUME_DIFFERENT_OWNER", OsStr::new("1"))
}

struct OperatorConfig {
    home: PathBuf,
    file: PathBuf,
}

fn operator_config(dir: &Path, contents: &str) -> OperatorConfig {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let file = home.join(".gitconfig");
    std::fs::write(&file, contents).unwrap();
    OperatorConfig { home, file }
}

fn routes_to(config: &OperatorConfig) -> [(&'static str, &OsStr); 3] {
    [
        ("GIT_CONFIG_GLOBAL", config.file.as_os_str()),
        ("GIT_CONFIG_SYSTEM", config.file.as_os_str()),
        ("HOME", config.home.as_os_str()),
    ]
}

fn infer_owners_under(repo: &Path, environment: &[(&str, &OsStr)]) -> std::process::Output {
    let mut infer = aoa();
    infer
        .args(["policy", "infer-owners", "--json", "--repo"])
        .arg(repo)
        .env_remove("XDG_CONFIG_HOME")
        .envs(environment.iter().copied());
    infer.output().expect("run")
}

#[test]
fn infer_owners_answers_the_same_when_operator_config_ignores_revisions_from_a_missing_file() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    repository_owned_by(ALICE, &repo);
    let missing = dir.path().join("no-such-ignore-revs");
    let config = operator_config(
        dir.path(),
        &format!("[blame]\n\tignoreRevsFile = {}\n", missing.display()),
    );
    let uninfluenced = inferred_owners(&repo, None);
    assert!(
        uninfluenced.is_some(),
        "infer-owners fails before any operator config is in play"
    );

    let changed_the_answer: Vec<&str> = routes_to(&config)
        .into_iter()
        .filter(|route| inferred_owners(&repo, Some(*route)) != uninfluenced)
        .map(|(variable, _)| variable)
        .collect();

    assert!(
        changed_the_answer.is_empty(),
        "operator git config reached through these variables changed what infer-owners \
reports: {changed_the_answer:?}"
    );
    assert!(!missing.exists());
}

#[test]
fn infer_owners_reads_a_distrusted_repository_the_operator_config_marks_safe() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().canonicalize().unwrap().join("repo");
    repository_owned_by(ALICE, &repo);
    let config = operator_config(
        dir.path(),
        &format!("[safe]\n\tdirectory = {}\n", repo.display()),
    );
    let trusted = inferred_owners(&repo, None).expect("infer-owners succeeds when git trusts");

    for route in routes_to(&config) {
        let output = infer_owners_under(&repo, &[ownership_distrusted(), route]);
        assert!(
            output.status.success(),
            "safe.directory reached through {} was not honoured: {}",
            route.0,
            String::from_utf8_lossy(&output.stderr)
        );
        let answer: Value = serde_json::from_slice(&output.stdout).expect("valid json");
        assert_eq!(answer, trusted, "route {}", route.0);
    }
}

#[test]
fn infer_owners_reports_git_refusing_a_distrusted_repository_no_operator_config_marks_safe() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().canonicalize().unwrap().join("repo");
    repository_owned_by(ALICE, &repo);
    let elsewhere = dir.path().join("elsewhere");
    let config = operator_config(
        dir.path(),
        &format!("[safe]\n\tdirectory = {}\n", elsewhere.display()),
    );

    let output = infer_owners_under(
        &repo,
        &[
            ownership_distrusted(),
            ("GIT_CONFIG_GLOBAL", config.file.as_os_str()),
            ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
        ],
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("dubious ownership"),
        "git's own refusal does not reach the operator: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
