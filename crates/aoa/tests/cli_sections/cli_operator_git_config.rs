use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;

use super::cli_git_environment::{
    inferred_owners, mined_clones, mined_corpus_report, repository_owned_by,
    revert_rate_correlations, ALICE,
};
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

fn only_this_global_config(config: &OperatorConfig) -> [(&'static str, &OsStr); 3] {
    [
        ownership_distrusted(),
        ("GIT_CONFIG_GLOBAL", config.file.as_os_str()),
        ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
    ]
}

fn infer_owners_reads(repo: &Path, started_in: &Path, environment: &[(&str, &OsStr)]) -> bool {
    let mut infer = aoa();
    infer
        .args(["policy", "infer-owners", "--json", "--repo"])
        .arg(repo)
        .current_dir(started_in)
        .env_remove("XDG_CONFIG_HOME")
        .envs(environment.iter().copied());
    infer.output().expect("run").status.success()
}

fn plain_git_reads(repo: &Path, started_in: &Path, environment: &[(&str, &OsStr)]) -> bool {
    let mut git = Command::new("git");
    git.arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .current_dir(started_in)
        .env_remove("XDG_CONFIG_HOME")
        .envs(environment.iter().copied());
    git.output().expect("run git").status.success()
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

fn plain_git_logs_reverts(repo: &Path, config: &OperatorConfig) -> bool {
    let mut git = Command::new("git");
    git.arg("-C")
        .arg(repo)
        .args(["log", "--all", "--grep=This reverts commit", "--format=%b"])
        .env("GIT_CONFIG_GLOBAL", &config.file);
    git.output().expect("run git").status.success()
}

#[test]
fn mine_corpus_answers_the_same_when_operator_config_sets_a_date_format_git_log_rejects() {
    let clones = mined_clones(true, false);
    let config = operator_config(clones.dir.path(), "[log]\n\tdate = no-such-format\n");
    assert!(
        !plain_git_logs_reverts(&clones.dir.path().join("clones/one"), &config),
        "plain git log succeeds under the operator config, so it breaks nothing"
    );
    let uninfluenced = mined_corpus_report(&clones, None).expect("mine-corpus succeeds");
    assert!(
        revert_rate_correlations(&uninfluenced) > 0,
        "the fixture's reverts do not reach the report: {uninfluenced}"
    );

    let changed_the_answer: Vec<&str> = routes_to(&config)
        .into_iter()
        .filter(|route| mined_corpus_report(&clones, Some(*route)).as_ref() != Some(&uninfluenced))
        .map(|(variable, _)| variable)
        .collect();

    assert!(
        changed_the_answer.is_empty(),
        "operator git config reached through these variables changed what mine-corpus \
reports: {changed_the_answer:?}"
    );
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

#[test]
fn infer_owners_trusts_exactly_the_repositories_plain_git_trusts_wherever_it_is_started() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let trusted_side = root.join("a");
    let other_side = root.join("b");
    repository_owned_by(ALICE, &trusted_side);
    repository_owned_by(ALICE, &other_side);
    let everything_is_safe = root.join("everything-is-safe");
    std::fs::write(&everything_is_safe, "[safe]\n\tdirectory = *\n").unwrap();
    let named_outright = format!("[safe]\n\tdirectory = {}\n", trusted_side.display());
    let widened_inside_the_trusted_side = format!(
        "{named_outright}[includeIf \"gitdir:{}/.git\"]\n\tpath = {}\n",
        trusted_side.display(),
        everything_is_safe.display()
    );

    let withdrawn_by_an_empty_value = format!("{named_outright}\tdirectory = \"\"\n");

    for (trust, contents) in [
        ("named outright", named_outright),
        (
            "named outright, then withdrawn by an empty value",
            withdrawn_by_an_empty_value,
        ),
        (
            "named outright, widened by a conditional include",
            widened_inside_the_trusted_side,
        ),
    ] {
        let config = operator_config(&root, &contents);
        let environment = only_this_global_config(&config);
        for repo in [&trusted_side, &other_side] {
            assert_eq!(
                infer_owners_reads(repo, &trusted_side, &environment),
                plain_git_reads(repo, &trusted_side, &environment),
                "infer-owners and plain git disagree on whether {} may be read \
(trust given by {trust}, both started in {})",
                repo.display(),
                trusted_side.display()
            );
        }
    }
}

#[test]
fn infer_owners_reads_a_distrusted_repository_listed_after_fifty_thousand_others() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().canonicalize().unwrap().join("repo");
    repository_owned_by(ALICE, &repo);
    let padding = "synthetic-checkout-".repeat(6);
    let mut contents = String::from("[safe]\n");
    for entry in 0..50_000 {
        contents.push_str(&format!("\tdirectory = /srv/{padding}{entry}\n"));
    }
    contents.push_str(&format!("\tdirectory = {}\n", repo.display()));
    let config = operator_config(dir.path(), &contents);
    let trusted = inferred_owners(&repo, None).expect("infer-owners succeeds when git trusts");

    let output = infer_owners_under(&repo, &only_this_global_config(&config));

    assert!(
        output.status.success(),
        "a long trust list stopped infer-owners reading a repository on it: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(answer, trusted);
}

#[test]
fn infer_owners_reads_the_named_repository_when_started_beside_a_corrupt_gitfile() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    repository_owned_by(ALICE, &repo);
    let started_in = dir.path().join("started-in");
    std::fs::create_dir_all(&started_in).unwrap();
    let corrupt_gitfile = started_in.join(".git");
    std::fs::write(&corrupt_gitfile, "this is not a gitfile\n").unwrap();
    let elsewhere = inferred_owners(&repo, None).expect("infer-owners succeeds elsewhere");

    let mut infer = aoa();
    infer
        .args(["policy", "infer-owners", "--json", "--repo"])
        .arg(&repo)
        .current_dir(&started_in);
    let output = infer.output().expect("run");

    assert!(
        output.status.success(),
        "a corrupt .git where infer-owners was started stopped it reading --repo: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(answer, elsewhere);
    assert_eq!(
        std::fs::read_to_string(&corrupt_gitfile).unwrap(),
        "this is not a gitfile\n"
    );
}

#[test]
fn infer_owners_leaves_nothing_behind_in_the_temporary_directory() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().canonicalize().unwrap().join("repo");
    repository_owned_by(ALICE, &repo);
    let config = operator_config(
        dir.path(),
        &format!("[safe]\n\tdirectory = {}\n", repo.display()),
    );
    let temporary = temporary_directory_with_mode(dir.path(), "temporary", 0o700);
    let mut environment = only_this_global_config(&config).to_vec();
    environment.push(("TMPDIR", temporary.as_os_str()));

    let output = infer_owners_under(&repo, &environment);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let left_behind = entries_of(&temporary);
    assert!(
        left_behind.is_empty(),
        "infer-owners left these in the temporary directory: {left_behind:?}"
    );
}

fn entries_of(directory: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    entries
}

#[test]
fn infer_owners_reads_a_trusted_repository_when_the_temporary_directory_is_a_relative_path() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("repo");
    repository_owned_by(ALICE, &repo);
    let config = operator_config(
        &root,
        &format!("[safe]\n\tdirectory = {}\n", repo.display()),
    );
    let trusted = inferred_owners(&repo, None).expect("infer-owners succeeds when git trusts");
    let started_in = root.join("started-in");
    std::fs::create_dir(&started_in).unwrap();
    let temporary = temporary_directory_with_mode(&started_in, "temporary", 0o700);
    let in_the_repository_before = entries_of(&repo);

    let mut infer = aoa();
    infer
        .args(["policy", "infer-owners", "--json", "--repo"])
        .arg(&repo)
        .current_dir(&started_in)
        .env_remove("XDG_CONFIG_HOME")
        .envs(only_this_global_config(&config))
        .env("TMPDIR", "temporary");
    let output = infer.output().expect("run");

    assert!(
        output.status.success(),
        "a relative TMPDIR stopped infer-owners reading a repository the operator trusts: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(answer, trusted);
    assert_eq!(entries_of(&temporary), Vec::<PathBuf>::new());
    assert_eq!(entries_of(&repo), in_the_repository_before);
}

fn refused_naming(output: &std::process::Output, named: &str) -> bool {
    !output.status.success()
        && output.stdout.is_empty()
        && String::from_utf8_lossy(&output.stderr).contains(named)
}

#[test]
fn infer_owners_refuses_a_relative_global_config_that_withdraws_trust_inside_the_repository() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("repo");
    repository_owned_by(ALICE, &repo);
    let everything_is_safe = root.join("system.gitconfig");
    std::fs::write(&everything_is_safe, "[safe]\n\tdirectory = *\n").unwrap();
    let withdrawal = "[safe]\n\tdirectory = \"\"\n";
    std::fs::write(repo.join("operator.gitconfig"), withdrawal).unwrap();
    let environment = [
        ownership_distrusted(),
        ("GIT_CONFIG_SYSTEM", everything_is_safe.as_os_str()),
        ("GIT_CONFIG_GLOBAL", OsStr::new("operator.gitconfig")),
    ];
    assert!(
        !plain_git_reads(&repo, &root, &environment),
        "plain git reads the repository, so the relative config did not withdraw trust"
    );

    let output = infer_owners_under(&repo, &environment);

    assert!(
        refused_naming(&output, "GIT_CONFIG_GLOBAL"),
        "infer-owners did not refuse naming GIT_CONFIG_GLOBAL: status {:?}, stdout {:?}, \
stderr {:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("operator.gitconfig")).unwrap(),
        withdrawal
    );
}

#[test]
fn infer_owners_refuses_each_config_locating_variable_set_to_a_relative_path() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    repository_owned_by(ALICE, &repo);

    let not_refused: Vec<&str> = [
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
        "HOME",
        "XDG_CONFIG_HOME",
    ]
    .into_iter()
    .filter(|variable| {
        let output = infer_owners_under(&repo, &[(variable, OsStr::new("relative/place"))]);
        !refused_naming(&output, variable)
    })
    .collect();

    assert!(
        not_refused.is_empty(),
        "infer-owners ran with these variables set to a relative path: {not_refused:?}"
    );
}

#[test]
fn infer_owners_reads_when_a_config_locating_variable_is_set_but_empty() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    repository_owned_by(ALICE, &repo);

    let output = infer_owners_under(&repo, &[("XDG_CONFIG_HOME", OsStr::new(""))]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temporary_directory_with_mode(parent: &Path, name: &str, mode: u32) -> PathBuf {
    let directory = parent.join(name);
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(mode)).unwrap();
    directory
}

#[test]
fn infer_owners_refuses_a_temporary_directory_others_can_replace_files_in() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("repo");
    repository_owned_by(ALICE, &repo);
    let expected = inferred_owners(&repo, None).expect("infer-owners succeeds by default");

    for (name, mode) in [("sticky", 0o1777), ("private", 0o700)] {
        let temporary = temporary_directory_with_mode(&root, name, mode);
        let output = infer_owners_under(&repo, &[("TMPDIR", temporary.as_os_str())]);
        assert!(
            output.status.success(),
            "a temporary directory with mode {mode:o} was refused: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let answer: Value = serde_json::from_slice(&output.stdout).expect("valid json");
        assert_eq!(answer, expected, "mode {mode:o}");
    }

    for (name, mode) in [
        ("world-writable", 0o777),
        ("group-writable", 0o770),
        ("other-writable", 0o702),
    ] {
        let temporary = temporary_directory_with_mode(&root, name, mode);
        let output = infer_owners_under(&repo, &[("TMPDIR", temporary.as_os_str())]);
        assert!(
            refused_naming(&output, &temporary.display().to_string()),
            "a temporary directory with mode {mode:o} was not refused by name: status {:?}, \
stdout {:?}, stderr {:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_dir(&temporary).unwrap().count(),
            0,
            "something was created in the refused directory (mode {mode:o})"
        );
    }
}
