use super::falsify_policy::{init_git_repo, run_git};
use super::*;

// --- aoa-hal.6: R16 ownership inference + R17 dual-register uniformity --------

/// A two-author git repo: alice owns `a/`, bob owns `b/` and the root file.
fn blame_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/one.txt"), "line\nline\nline\n").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(
        dir.path(),
        &[
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@example.com",
            "commit",
            "-qm",
            "alice adds a/",
        ],
    );
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    std::fs::write(dir.path().join("b/two.txt"), "x\n").unwrap();
    std::fs::write(dir.path().join("ROOT.md"), "root\n").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(
        dir.path(),
        &[
            "-c",
            "user.name=Bob",
            "-c",
            "user.email=bob@example.com",
            "commit",
            "-qm",
            "bob adds b/ and root",
        ],
    );
    dir
}

// R16 AC: infer-owners emits a reviewable CODEOWNERS diff and never writes
// without --write.
#[test]
fn infer_owners_prints_reviewable_diff_without_writing() {
    let repo = blame_repo();
    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("create: .github/CODEOWNERS"))
        .stdout(predicate::str::contains("+/a/ alice@example.com"))
        .stdout(predicate::str::contains("+/b/ bob@example.com"))
        .stdout(predicate::str::contains("+/* bob@example.com"));
    assert!(
        !repo.path().join(".github/CODEOWNERS").exists(),
        "default run must not write CODEOWNERS"
    );
}

#[test]
fn infer_owners_write_writes_the_proposal() {
    let repo = blame_repo();
    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--write",
        ])
        .assert()
        .success();
    let owners = std::fs::read_to_string(repo.path().join(".github/CODEOWNERS")).unwrap();
    assert!(owners.starts_with("# PROPOSED by `aoa policy infer-owners`"));
    assert!(owners.contains("/a/ alice@example.com\n"));
    assert!(owners.contains("/b/ bob@example.com\n"));
}

// R17 AC: the JSON register carries the same findings as the human diff.
#[test]
fn infer_owners_json_matches_human_findings() {
    let repo = blame_repo();
    let output = aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["written"], false);
    let entries = parsed["entries"].as_array().expect("entries array");
    let alice = entries
        .iter()
        .find(|e| e["pattern"] == "/a/")
        .expect("/a/ entry");
    assert_eq!(alice["owner"], "alice@example.com");
    assert_eq!(alice["owned_lines"], 3);
    assert_eq!(alice["total_lines"], 3);
    // Identical findings across registers: every JSON entry appears in the diff.
    let diff = parsed["diff"].as_str().expect("diff string");
    for entry in entries {
        let line = format!(
            "+{} {}",
            entry["pattern"].as_str().unwrap(),
            entry["owner"].as_str().unwrap()
        );
        assert!(diff.contains(&line), "JSON entry missing from diff: {line}");
    }
}

// R16: enumeration and attribution share HEAD as the source of truth — a
// staged-but-uncommitted file (a routine mid-task index state) must not abort
// the command with a failed `git blame HEAD` on a path HEAD has never seen.
#[test]
fn infer_owners_ignores_staged_but_uncommitted_files() {
    let repo = blame_repo();
    std::fs::create_dir_all(repo.path().join("staged")).unwrap();
    std::fs::write(repo.path().join("staged/new.txt"), "uncommitted\n").unwrap();
    run_git(repo.path(), &["add", "staged/new.txt"]);
    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("+/a/ alice@example.com"))
        .stdout(predicate::str::contains("/staged/").not());
}

// R16: a merge-conflicted file lists one index entry per stage (1/2/3); its
// lines must be counted once, not once per stage, or its authors gain 3x
// weight and the reported arithmetic is silently wrong.
#[test]
fn infer_owners_counts_conflicted_files_once() {
    let repo = TempDir::new().unwrap();
    init_git_repo(repo.path());
    std::fs::create_dir_all(repo.path().join("c")).unwrap();
    std::fs::write(repo.path().join("c/f.txt"), "one\ntwo\n").unwrap();
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "-qm", "base"]);
    run_git(repo.path(), &["checkout", "-qb", "side"]);
    std::fs::write(repo.path().join("c/f.txt"), "one\ntwo side\n").unwrap();
    run_git(repo.path(), &["commit", "-aqm", "side edit"]);
    run_git(repo.path(), &["checkout", "-q", "-"]);
    std::fs::write(repo.path().join("c/f.txt"), "one\ntwo main\n").unwrap();
    run_git(repo.path(), &["commit", "-aqm", "main edit"]);
    // The merge conflicts by construction; leave the index unmerged.
    let merge = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["merge", "-q", "side"])
        .output()
        .expect("git available");
    assert!(!merge.status.success(), "merge must conflict");

    let output = aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let entries = parsed["entries"].as_array().expect("entries array");
    let c = entries
        .iter()
        .find(|e| e["pattern"] == "/c/")
        .expect("/c/ entry");
    assert_eq!(
        c["total_lines"], 2,
        "2-line file must not be triple-counted"
    );
    assert_eq!(c["owned_lines"], 2);
}

// R17: with zero attributed entries both registers carry nothing actionable —
// the JSON must not advertise a create-diff that the human register omits and
// that --write refuses.
#[test]
fn infer_owners_zero_entries_keeps_registers_in_parity() {
    let repo = TempDir::new().unwrap();
    init_git_repo(repo.path());
    run_git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "empty"],
    );

    aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to propose"));

    let output = aoa()
        .args([
            "policy",
            "infer-owners",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["entries"], serde_json::json!([]));
    assert_eq!(
        parsed["proposal"], "",
        "no proposal content without entries"
    );
    assert_eq!(parsed["diff"], "", "no diff the human register never shows");
}

// R17: policy compile exposes the JSON register listing the written planes.
#[test]
fn policy_compile_json_lists_written_planes() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\n",
    )
    .unwrap();
    let output = aoa()
        .args([
            "policy",
            "compile",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let planes: Vec<&str> = parsed["planes_written"]
        .as_array()
        .expect("planes_written array")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert!(planes
        .iter()
        .any(|p| p.ends_with(".pre-commit-config.yaml")));
    assert!(planes.iter().any(|p| p.ends_with("CODEOWNERS")));
}

// R17: guard-staged exposes the JSON register carrying the blocked findings and
// keeps the failure exit code.
#[test]
fn policy_guard_staged_json_carries_blocked_findings() {
    let repo = TempDir::new().unwrap();
    std::fs::write(
        repo.path().join("aoa-policy.yaml"),
        "protected_paths: [\"migrations/**\"]\n",
    )
    .unwrap();

    let output = aoa()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
            "migrations/0001.sql",
            "src/lib.rs",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(
        parsed["blocked"],
        serde_json::json!(["migrations/0001.sql"])
    );

    let clean = aoa()
        .args([
            "policy",
            "guard-staged",
            "--repo",
            repo.path().to_str().unwrap(),
            "--json",
            "src/lib.rs",
        ])
        .output()
        .expect("run");
    assert!(clean.status.success());
    let parsed: Value = serde_json::from_slice(&clean.stdout).expect("valid json");
    assert_eq!(parsed["blocked"], serde_json::json!([]));
}

// R17: observe exposes the JSON register reporting the installed paths.
#[test]
fn observe_json_reports_installed_paths() {
    let repo = TempDir::new().unwrap();
    let output = aoa()
        .args(["observe", "--repo", repo.path().to_str().unwrap(), "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed["traces_dir"].as_str().unwrap().ends_with("traces"));
    assert!(parsed["gitignore"].is_string());
    assert!(parsed["enforce_settings"].is_null());
}
