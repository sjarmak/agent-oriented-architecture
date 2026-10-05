use std::ffi::OsStr;

use super::audit_git_environment::{
    INHERITED_REPOSITORY_VARIABLES, INHERITED_TRACE_VARIABLES, STANDARD_OUTPUT,
};
use super::falsify_policy::{init_git_repo, run_git};
use super::*;

pub(super) const ALICE: [&str; 4] = [
    "-c",
    "user.name=Alice",
    "-c",
    "user.email=alice@example.com",
];

const CAROL: [&str; 4] = [
    "-c",
    "user.name=Carol",
    "-c",
    "user.email=carol@example.com",
];

pub(super) type Inherited<'a> = Option<(&'a str, &'a OsStr)>;

pub(super) fn json_answer(mut command: Command, inherited: Inherited<'_>) -> Option<Value> {
    if let Some((variable, value)) = inherited {
        command.env(variable, value);
    }
    let output = command.output().expect("run");
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).expect("valid json"))
}

fn variables_that_change_the_answer<'a>(
    variables: &[&'a str],
    value: &OsStr,
    answer: impl Fn(Inherited<'_>) -> Option<Value>,
) -> Vec<&'a str> {
    let uninfluenced = answer(None);
    assert!(
        uninfluenced.is_some(),
        "the command fails before any variable is inherited"
    );
    variables
        .iter()
        .copied()
        .filter(|variable| answer(Some((variable, value))) != uninfluenced)
        .collect()
}

fn commit_all(repo: &Path, author: [&str; 4], message: &str) {
    run_git(repo, &["add", "."]);
    let mut args = author.to_vec();
    args.extend(["commit", "-qm", message]);
    run_git(repo, &args);
}

pub(super) fn repository_owned_by(author: [&str; 4], root: &Path) {
    std::fs::create_dir_all(root.join("a")).unwrap();
    init_git_repo(root);
    std::fs::write(root.join("a/one.txt"), "line\nline\nline\n").unwrap();
    std::fs::write(root.join("ROOT.md"), "root\n").unwrap();
    commit_all(root, author, "add a/ and root");
}

pub(super) fn inferred_owners(repo: &Path, inherited: Inherited<'_>) -> Option<Value> {
    let mut infer = aoa();
    infer
        .args(["policy", "infer-owners", "--json", "--repo"])
        .arg(repo);
    json_answer(infer, inherited)
}

#[test]
fn infer_owners_lists_the_named_repository_under_every_inherited_repository_variable() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    let ambient = dir.path().join("ambient");
    repository_owned_by(ALICE, &repo);
    run_git(
        dir.path(),
        &["clone", "-q", "--template=", "repo", "ambient"],
    );
    std::fs::write(ambient.join("a/only-in-ambient.txt"), "line\n").unwrap();
    commit_all(&ambient, ALICE, "add a file the named repository lacks");

    let changed_the_answer = variables_that_change_the_answer(
        &INHERITED_REPOSITORY_VARIABLES,
        ambient.join(".git").as_os_str(),
        |inherited| inferred_owners(&repo, inherited),
    );

    assert!(
        changed_the_answer.is_empty(),
        "the caller's environment changed which files infer-owners lists: {changed_the_answer:?}"
    );
}

#[test]
fn infer_owners_blames_the_named_repository_under_every_inherited_repository_variable() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    let ambient = dir.path().join("ambient");
    repository_owned_by(ALICE, &repo);
    repository_owned_by(CAROL, &ambient);

    let changed_the_answer = variables_that_change_the_answer(
        &INHERITED_REPOSITORY_VARIABLES,
        ambient.join(".git").as_os_str(),
        |inherited| inferred_owners(&repo, inherited),
    );

    assert!(
        changed_the_answer.is_empty(),
        "the caller's environment changed who infer-owners attributes lines to: \
{changed_the_answer:?}"
    );
}

#[test]
fn infer_owners_answers_the_same_when_the_caller_traces_git_to_standard_output() {
    let dir = TempDir::new().expect("tempdir");
    repository_owned_by(ALICE, dir.path());

    let changed_the_answer = variables_that_change_the_answer(
        &INHERITED_TRACE_VARIABLES,
        OsStr::new(STANDARD_OUTPUT),
        |inherited| inferred_owners(dir.path(), inherited),
    );

    assert!(
        changed_the_answer.is_empty(),
        "the caller's git tracing changed what infer-owners reports: {changed_the_answer:?}"
    );
}

pub(super) struct MinedClones {
    pub(super) dir: TempDir,
    ambient: PathBuf,
}

const CLONES_WITH_A_LOCKFILE: [(&str, bool); 4] = [
    ("one", false),
    ("two", true),
    ("three", true),
    ("four", false),
];

fn head_commit(repo: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git available");
    assert!(output.status.success(), "git rev-parse HEAD failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(super) fn mined_clones(
    revert_the_unlocked: bool,
    ambient_holds_the_mined_commits: bool,
) -> MinedClones {
    let dir = TempDir::new().expect("tempdir");
    let ambient = dir.path().join("ambient");
    std::fs::create_dir_all(&ambient).unwrap();
    init_git_repo(&ambient);
    std::fs::write(ambient.join("ambient.txt"), "ambient\n").unwrap();
    commit_all(&ambient, CAROL, "ambient history");

    for (name, has_a_lockfile) in CLONES_WITH_A_LOCKFILE {
        let clone = dir.path().join("clones").join(name);
        std::fs::create_dir_all(&clone).unwrap();
        init_git_repo(&clone);
        std::fs::write(clone.join("main.py"), "print('base')\n").unwrap();
        if has_a_lockfile {
            std::fs::write(clone.join("Cargo.lock"), "\n").unwrap();
        }
        commit_all(&clone, ALICE, "base");
        std::fs::write(clone.join("main.py"), "print('changed')\n").unwrap();
        commit_all(&clone, ALICE, "change");
        let mined = head_commit(&clone);
        if ambient_holds_the_mined_commits {
            run_git(&ambient, &["fetch", "-q", clone.to_str().unwrap(), "HEAD"]);
        }
        if revert_the_unlocked && !has_a_lockfile {
            let reverts = format!("This reverts commit {mined}.");
            let mut args = ALICE.to_vec();
            args.extend([
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Revert",
                "-m",
                &reverts,
            ]);
            run_git(&clone, &args);
        }

        let task = dir.path().join("tasks").join(name);
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(
            task.join("metadata.json"),
            serde_json::json!({
                "id": name,
                "repo": name,
                "metadata": { "ground_truth_commit": mined },
                "verification": { "oracle_answer": ["main.py"] },
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(task.join("instruction.md"), "change main.py\n").unwrap();
    }
    MinedClones { dir, ambient }
}

pub(super) fn mined_corpus_report(clones: &MinedClones, inherited: Inherited<'_>) -> Option<Value> {
    let root = clones.dir.path();
    let mut mine = aoa();
    mine.args(["gap", "mine-corpus", "--json", "--tasks"])
        .arg(root.join("tasks"))
        .arg("--clones")
        .arg(root.join("clones"))
        .arg("--out")
        .arg(root.join("report.json"));
    json_answer(mine, inherited)
}

pub(super) fn revert_rate_correlations(report: &Value) -> usize {
    report["construct"]["metrics"]
        .as_array()
        .expect("metrics")
        .iter()
        .map(|metric| {
            metric["correlations"]
                .as_array()
                .expect("correlations")
                .len()
        })
        .sum()
}

#[test]
fn mine_corpus_resolves_commits_in_the_named_clone_under_every_inherited_repository_variable() {
    let clones = mined_clones(false, false);

    let changed_the_answer = variables_that_change_the_answer(
        &INHERITED_REPOSITORY_VARIABLES,
        clones.ambient.join(".git").as_os_str(),
        |inherited| mined_corpus_report(&clones, inherited),
    );

    assert!(
        changed_the_answer.is_empty(),
        "the caller's environment changed which commits mine-corpus resolves: \
{changed_the_answer:?}"
    );
}

#[test]
fn mine_corpus_reads_reverts_from_the_named_clone_under_every_inherited_repository_variable() {
    let clones = mined_clones(true, true);
    let uninfluenced = mined_corpus_report(&clones, None).expect("mine-corpus succeeds");
    assert!(
        revert_rate_correlations(&uninfluenced) > 0,
        "the fixture's reverts do not reach the report: {uninfluenced}"
    );

    let changed_the_answer = variables_that_change_the_answer(
        &INHERITED_REPOSITORY_VARIABLES,
        clones.ambient.join(".git").as_os_str(),
        |inherited| mined_corpus_report(&clones, inherited),
    );

    assert!(
        changed_the_answer.is_empty(),
        "the caller's environment changed which reverts mine-corpus counts: \
{changed_the_answer:?}"
    );
}

#[test]
fn mine_corpus_answers_the_same_when_the_caller_traces_git_to_standard_output() {
    let clones = mined_clones(true, false);

    let changed_the_answer = variables_that_change_the_answer(
        &INHERITED_TRACE_VARIABLES,
        OsStr::new(STANDARD_OUTPUT),
        |inherited| mined_corpus_report(&clones, inherited),
    );

    assert!(
        changed_the_answer.is_empty(),
        "the caller's git tracing changed what mine-corpus reports: {changed_the_answer:?}"
    );
}
