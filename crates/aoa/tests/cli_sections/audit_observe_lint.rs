use super::falsify_policy::init_git_repo;
use super::*;

// Criterion 4: observe makes no tracked-file changes.
#[test]
fn observe_makes_no_tracked_changes() {
    let repo = TempDir::new().expect("tempdir");
    init_git_repo(repo.path());

    aoa()
        .args(["observe", "--repo"])
        .arg(repo.path())
        .assert()
        .success();

    let status = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    let porcelain = String::from_utf8_lossy(&status.stdout);
    // The only artifact is the explicitly-ignored .aoa/ tree, which carries its
    // own ignore guard, so the working tree stays clean.
    assert!(
        porcelain.trim().is_empty(),
        "working tree not clean: {porcelain}"
    );
}

// Criterion 5 + 9 (audit half): tiered punch-list, --json structured, --fail-on tier1.
#[test]
fn audit_human_prints_punch_list() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("punch-list"))
        .stdout(predicate::str::contains("tier-1"));
}

#[test]
fn audit_json_is_parseable() {
    let repo = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(parsed["items"].is_array());
}

#[cfg(unix)]
fn git_on_path() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH is set"))
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git is on PATH")
}

#[cfg(unix)]
fn path_led_by_a_git_whose_descendant_holds_stdout(stand_in_dir: &Path, seconds: u64) -> String {
    use std::os::unix::fs::PermissionsExt;

    let stand_in = stand_in_dir.join("git");
    std::fs::write(
        &stand_in,
        format!(
            "#!/bin/sh\ncase \" $* \" in *\" --git-path \"*) sleep {seconds} & ;; esac\nexec '{}' \"$@\"\n",
            git_on_path().display()
        ),
    )
    .expect("write the git stand-in");
    std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755))
        .expect("make the stand-in executable");
    let inherited = std::env::var("PATH").expect("PATH is utf-8");
    format!("{}:{inherited}", stand_in_dir.display())
}

#[cfg(unix)]
#[test]
fn audit_fails_within_the_git_deadline_when_a_git_descendant_holds_stdout_open() {
    const DESCENDANT_HOLDS_STDOUT_SECONDS: u64 = 25;
    const LONGEST_ACCEPTED_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
    let repo = TempDir::new().expect("tempdir");
    init_git_repo(repo.path());
    let stand_in_dir = TempDir::new().expect("stand-in dir");
    let path = path_led_by_a_git_whose_descendant_holds_stdout(
        stand_in_dir.path(),
        DESCENDANT_HOLDS_STDOUT_SECONDS,
    );

    let started = std::time::Instant::now();
    let output = aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .env("PATH", path)
        .output()
        .expect("run");
    let waited = started.elapsed();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "audit succeeded: {stderr}");
    assert!(stderr.contains("git did not answer within 10s"), "{stderr}");
    assert!(waited < LONGEST_ACCEPTED_WAIT, "audit took {waited:?}");
}

#[cfg(unix)]
#[test]
fn audit_says_why_git_could_not_be_asked_about_the_pre_commit_plane() {
    let repo = TempDir::new().expect("tempdir");
    init_git_repo(repo.path());
    let no_git_here = TempDir::new().expect("empty PATH directory");

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .env("PATH", no_git_here.path())
        .output()
        .expect("run");

    assert!(output.status.success(), "audit must not abort: {output:?}");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let titles: Vec<&str> = parsed["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|item| item["kind"] == "missing_plane")
        .map(|item| item["title"].as_str().expect("title"))
        .collect();
    assert_eq!(titles.len(), 3, "{titles:?}");
    assert!(
        titles.iter().any(|title| title
            .starts_with("missing enforcement plane: pre-commit hook (git could not be started: ")),
        "{titles:?}"
    );
}

// aoa-d6t.31 review follow-up: a repo whose workspace manifest is malformed
// must still get its full punch-list — the CLI degrades to repo-wide findings
// with the discovery failure surfaced, never an abort with no report.
#[test]
fn audit_degrades_on_malformed_workspace_manifest() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(repo.path().join("package.json"), "{ \"name\": \"x\", }").unwrap();
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(output.status.success(), "audit must not abort: {output:?}");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(!parsed["items"].as_array().expect("items").is_empty());
    assert!(parsed["subtree_discovery_warning"]
        .as_str()
        .expect("warning surfaced on the wire")
        .contains("package.json"));
}

#[test]
fn recommend_degrades_on_malformed_workspace_manifest() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::write(repo.path().join("package.json"), "{ \"name\": \"x\", }").unwrap();
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();

    let output = aoa()
        .args(["recommend", "--json", "--repo"])
        .arg(repo.path())
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "recommend must not abort: {output:?}"
    );
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(!parsed["items"].as_array().expect("items").is_empty());
    // The recommendation report has no warning field of its own; the CLI
    // surfaces the audit's degradation on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("package.json"),
        "discovery failure must be surfaced on stderr: {stderr}"
    );
}

#[test]
fn audit_fail_on_tier1_exits_non_zero_when_tier1_present() {
    // A bare repo is missing the runtime-hook and CI planes (both Tier-1).
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure();
}

#[test]
fn audit_fail_on_tier1_exits_zero_without_tier1_gap() {
    // Present the two Tier-1 planes (runtime hook + CI) so only the Tier-2
    // pre-commit plane is missing; --fail-on tier1 must then exit 0.
    //
    // The runtime plane is made *live* rather than merely registered, so the
    // fixture pins the strongest passing state. The two weaker ones are not
    // interchangeable with it: a traces directory that exists and stays empty is
    // a Tier-1 silence (aoa-dpluh) and would fail this gate, while an absent one
    // is the Tier-3 unobserved plane (aoa-rsixa) and would pass it — but pass on
    // a different fact than the one this test is pinning.
    let repo = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(
        repo.path().join(".claude/settings.json"),
        r#"{"hooks":{
            "PostToolUse":[
                {"matcher":"Bash","hooks":[{"command":"aoa enforce record"}]},
                {"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce commit"}]}
            ],
            "PreToolUse":[{"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce check"}]}],
            "PostToolUseFailure":[{"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce fail"}]}],
            "PermissionDenied":[{"matcher":"Write|Edit|MultiEdit|NotebookEdit","hooks":[{"command":"aoa enforce deny"}]}]
        }}"#,
    )
    .unwrap();
    std::fs::create_dir_all(repo.path().join(".github/workflows")).unwrap();
    std::fs::create_dir_all(repo.path().join(".aoa/traces")).unwrap();
    std::fs::write(
        repo.path().join(".aoa/traces/live-s1.jsonl"),
        "{\"type\":\"test.run\",\"source\":\"native\",\"seq\":0,\"attributes\":{}}\n",
    )
    .unwrap();

    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

#[test]
fn audit_fail_on_tier1_rejects_a_gutted_runtime_hook_file() {
    let repo = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
    std::fs::write(repo.path().join(".claude/settings.json"), "{}").unwrap();
    std::fs::create_dir_all(repo.path().join(".github/workflows")).unwrap();

    aoa()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "missing enforcement plane: runtime hook",
        ));
}

#[test]
fn audit_without_fail_on_exits_zero_even_with_tier1_gap() {
    let repo = TempDir::new().expect("tempdir");
    aoa()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

// Criterion 6: lint-context --changed flags only changed files and honors the
// oversized-context suppression marker.
#[test]
fn lint_context_changed_filters_and_honors_suppression() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("AGENTS.md");
    let changed = dir.path().join("changed.md");
    let other = dir.path().join("other.md");
    let suppressed = dir.path().join("suppressed.md");

    std::fs::write(
        &root,
        "# Root\n\nSee [changed](changed.md), [other](other.md), [suppressed](suppressed.md).\n",
    )
    .unwrap();

    let dup_section = format!("# Dup\n\nbody\n\n# Dup\n\n{}", "line\n".repeat(50));
    std::fs::write(&changed, &dup_section).unwrap();
    std::fs::write(&other, &dup_section).unwrap();
    std::fs::write(
        &suppressed,
        "# aoa-allow: oversized-context giant onboarding doc\n\n# Suppressed\n\nbody\n",
    )
    .unwrap();

    let output = aoa()
        .args(["lint-context", "--json", "--root"])
        .arg(&root)
        .arg("--changed")
        .arg(&changed)
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");

    let findings = parsed["findings"].as_array().expect("findings array");
    assert!(
        !findings.is_empty(),
        "expected findings for the changed file"
    );
    for finding in findings {
        let file = finding["file"].as_str().unwrap();
        assert!(
            file.ends_with("changed.md"),
            "finding leaked from a non-changed file: {file}"
        );
        assert!(
            !file.ends_with("other.md"),
            "finding leaked from other.md: {file}"
        );
    }

    let suppressions = parsed["suppressed"].as_array().expect("suppressed array");
    assert!(
        suppressions
            .iter()
            .any(|s| s["file"].as_str().unwrap().ends_with("suppressed.md")),
        "suppression marker not honored"
    );
}

#[test]
fn lint_context_human_renders_text() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("AGENTS.md");
    std::fs::write(&root, "# Root\n\nplain doc with no smells\n").unwrap();

    aoa()
        .args(["lint-context", "--root"])
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("context lint"));
}

const LINT_DUPLICATED_HEADING: &str = "# Rules\n\nfirst\n\n# Rules\n\nsecond\n";

fn lint_json(dir: &Path, args: &[&str]) -> Value {
    let output = aoa()
        .current_dir(dir)
        .args(["lint-context", "--json"])
        .args(args)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "lint-context failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

fn json_strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|entry| entry.as_str().expect("string"))
        .collect()
}

fn finding_files(parsed: &Value) -> Vec<&str> {
    parsed["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .map(|finding| finding["file"].as_str().expect("file"))
        .collect()
}

#[test]
fn lint_context_without_root_lints_a_claude_only_tree_with_nested_files() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("CLAUDE.md"), "# Root\n\nplain\n").unwrap();
    std::fs::create_dir_all(dir.path().join("services/api")).unwrap();
    std::fs::write(
        dir.path().join("services/api/CLAUDE.md"),
        LINT_DUPLICATED_HEADING,
    )
    .unwrap();

    let parsed = lint_json(dir.path(), &[]);

    assert_eq!(
        json_strings(&parsed["roots"]),
        ["CLAUDE.md", "services/api/CLAUDE.md"]
    );
    assert_eq!(finding_files(&parsed), ["services/api/CLAUDE.md"]);
}

#[test]
fn lint_context_explicit_root_also_lints_nested_context_files() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("CLAUDE.md"), "# Root\n\nplain\n").unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/AGENTS.md"), LINT_DUPLICATED_HEADING).unwrap();

    let parsed = lint_json(dir.path(), &["--root", "./CLAUDE.md"]);

    assert_eq!(
        json_strings(&parsed["roots"]),
        ["CLAUDE.md", "pkg/AGENTS.md"]
    );
    assert_eq!(finding_files(&parsed), ["pkg/AGENTS.md"]);
}

#[test]
fn lint_context_root_above_the_working_directory_lints_the_named_file() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), LINT_DUPLICATED_HEADING).unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/AGENTS.md"), "# Pkg\n\nplain\n").unwrap();

    let parsed = lint_json(&dir.path().join("pkg"), &["--root", "../AGENTS.md"]);

    assert_eq!(
        json_strings(&parsed["roots"]),
        ["../AGENTS.md", "../pkg/AGENTS.md"]
    );
    assert_eq!(finding_files(&parsed), ["../AGENTS.md"]);
}

fn write_package_linking_shared_docs(dir: &Path) {
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/shared.md"), "# Shared\n\nplain\n").unwrap();
    std::fs::write(
        dir.join("pkg/AGENTS.md"),
        "# Pkg\n\nSee [shared](../docs/shared.md).\n",
    )
    .unwrap();
}

fn closure_files(closure: &Value) -> Vec<&str> {
    closure["files"]
        .as_array()
        .expect("files array")
        .iter()
        .map(|file| file["file"].as_str().expect("file"))
        .collect()
}

#[test]
fn lint_context_counts_a_sibling_directory_for_the_only_nested_root() {
    let dir = TempDir::new().expect("tempdir");
    write_package_linking_shared_docs(dir.path());

    let parsed = lint_json(dir.path(), &[]);

    let closure = &parsed["budget"]["closures"][0];
    assert_eq!(closure_files(closure), ["pkg/AGENTS.md", "docs/shared.md"]);
    assert!(json_strings(&closure["outside_boundary"]).is_empty());
}

#[test]
fn lint_context_names_a_member_it_could_not_count() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "[binary](binary.md)\n").expect("write root");
    std::fs::write(dir.path().join("binary.md"), b"rules\n\xff\n").expect("write member");

    let parsed = lint_json(dir.path(), &[]);

    assert_eq!(
        parsed["budget"]["closures"][0]["unread"],
        serde_json::json!([{ "path": "binary.md", "reason": "not_utf8" }])
    );
    aoa()
        .current_dir(dir.path())
        .arg("lint-context")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "binary.md: not counted, it is not UTF-8 text",
        ));
}

#[test]
fn lint_context_names_a_link_that_leaves_the_directory_of_an_explicit_root() {
    let dir = TempDir::new().expect("tempdir");
    write_package_linking_shared_docs(dir.path());

    let parsed = lint_json(dir.path(), &["--root", "pkg/AGENTS.md"]);

    let closure = &parsed["budget"]["closures"][0];
    assert_eq!(closure_files(closure), ["pkg/AGENTS.md"]);
    assert_eq!(
        json_strings(&closure["outside_boundary"]),
        ["docs/shared.md"]
    );
    aoa()
        .current_dir(dir.path())
        .args(["lint-context", "--root", "pkg/AGENTS.md"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "docs/shared.md: not counted, it resolves outside the linted directory",
        ));
}

#[cfg(unix)]
pub(super) fn lint_outputs(dir: &Path) -> [std::process::Output; 2] {
    [&["lint-context", "--json"][..], &["lint-context"][..]].map(|args| {
        aoa()
            .current_dir(dir)
            .args(args)
            .output()
            .expect("run lint-context")
    })
}

#[cfg(unix)]
#[test]
fn lint_context_output_does_not_depend_on_what_a_link_leaving_the_directory_reaches() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).expect("create repo");
    std::fs::write(
        repo.join("AGENTS.md"),
        "[member](probe.md) [written](../written.md)\n",
    )
    .expect("write root");
    std::os::unix::fs::symlink("../outside.md", repo.join("probe.md")).expect("link the member");
    let outside = [dir.path().join("outside.md"), dir.path().join("written.md")];

    let while_absent = lint_outputs(&repo);
    for path in &outside {
        std::fs::write(path, "outside\n").expect("write outside file");
    }
    let while_files = lint_outputs(&repo);
    for path in &outside {
        std::fs::remove_file(path).expect("remove outside file");
        std::fs::create_dir(path).expect("create outside directory");
    }
    let while_directories = lint_outputs(&repo);

    assert_eq!(while_absent, while_files);
    assert_eq!(while_absent, while_directories);
    let json: Value = serde_json::from_slice(&while_absent[0].stdout).expect("valid json");
    let closure = &json["budget"]["closures"][0];
    assert_eq!(
        json_strings(&closure["outside_boundary"]),
        ["probe.md", "../written.md"]
    );
    assert_eq!(closure["unread"], serde_json::json!([]));
}

#[cfg(unix)]
#[test]
fn lint_context_output_does_not_depend_on_what_a_link_steps_through_outside_the_directory() {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).expect("create repo");
    std::fs::write(repo.join("AGENTS.md"), "[member](probe.md)\n").expect("write root");
    std::fs::write(repo.join("safe.md"), "safe\n").expect("write the file linked back to");
    std::os::unix::fs::symlink("../probed-dir/../repo/safe.md", repo.join("probe.md"))
        .expect("link the member");
    let probed = dir.path().join("probed-dir");

    let while_absent = lint_outputs(&repo);
    std::fs::create_dir(&probed).expect("create outside directory");
    let while_a_directory = lint_outputs(&repo);
    std::fs::remove_dir(&probed).expect("remove outside directory");
    std::fs::write(&probed, "outside\n").expect("write outside file");
    let while_a_file = lint_outputs(&repo);
    std::fs::remove_file(&probed).expect("remove outside file");
    std::os::unix::fs::symlink("removed", &probed).expect("plant a broken link outside");
    let while_a_broken_link = lint_outputs(&repo);

    assert_eq!(while_absent, while_a_directory);
    assert_eq!(while_absent, while_a_file);
    assert_eq!(while_absent, while_a_broken_link);
    let json: Value = serde_json::from_slice(&while_absent[0].stdout).expect("valid json");
    let closure = &json["budget"]["closures"][0];
    assert_eq!(closure_files(closure), ["AGENTS.md"]);
    assert_eq!(json_strings(&closure["outside_boundary"]), ["probe.md"]);
    assert_eq!(closure["unread"], serde_json::json!([]));
    assert_eq!(json["findings"], serde_json::json!([]));
}

#[test]
fn lint_context_human_reports_how_many_roots_were_linted() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("AGENTS.md"), "# Root\n").unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/CLAUDE.md"), "# Pkg\n").unwrap();

    aoa()
        .current_dir(dir.path())
        .arg("lint-context")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "context lint: 0 finding(s) across 2 context root(s)",
        ));
}

const LINT_BUDGET_ROOT: &str = "# Root\n\nSee [shared](shared.md).\n";
const LINT_BUDGET_MEMBER: &str = "# Member\n\nSee [shared](../shared.md) for the member rules.\n";
const LINT_BUDGET_SHARED: &str = "# Shared\n\nalpha beta gamma delta epsilon\n";

fn lint_budget_tree() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("CLAUDE.md"), LINT_BUDGET_ROOT).unwrap();
    std::fs::write(dir.path().join("shared.md"), LINT_BUDGET_SHARED).unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/AGENTS.md"), LINT_BUDGET_MEMBER).unwrap();
    dir
}

fn lint_budget_tokens(encoding: &str, texts: &[&str]) -> u64 {
    let encoder = aoa_budget::target_encoder(encoding).expect("encoder");
    texts
        .iter()
        .map(|text| aoa_budget::count_tokens(&encoder, text) as u64)
        .sum()
}

#[test]
fn lint_context_json_reports_tokens_for_the_root_closure_and_each_member() {
    let dir = lint_budget_tree();

    let parsed = lint_json(dir.path(), &[]);

    let root = lint_budget_tokens("o200k_base", &[LINT_BUDGET_ROOT, LINT_BUDGET_SHARED]);
    let member = lint_budget_tokens("o200k_base", &[LINT_BUDGET_MEMBER, LINT_BUDGET_SHARED]);
    let distinct = lint_budget_tokens(
        "o200k_base",
        &[LINT_BUDGET_ROOT, LINT_BUDGET_SHARED, LINT_BUDGET_MEMBER],
    );
    let file = |path: &str, text: &str| {
        let tokens = lint_budget_tokens("o200k_base", &[text]);
        serde_json::json!({
            "file": path,
            "target_tokens": tokens,
            "o200k_tokens": tokens,
            "gating": true,
        })
    };
    assert!(root > 0 && member > 0);
    assert_eq!(
        parsed["budget"],
        serde_json::json!({
            "tokenizer": "o200k_base",
            "reference_encoding": "o200k_base",
            "ceiling": 2000,
            "distinct_files": 3,
            "target_tokens": distinct,
            "o200k_tokens": distinct,
            "closures": [
                {
                    "root": "CLAUDE.md",
                    "target_tokens": root,
                    "o200k_tokens": root,
                    "gating_target_tokens": root,
                    "over_ceiling_by": 0,
                    "files": [
                        file("CLAUDE.md", LINT_BUDGET_ROOT),
                        file("shared.md", LINT_BUDGET_SHARED),
                    ],
                    "outside_boundary": [],
                    "unread": [],
                },
                {
                    "root": "pkg/AGENTS.md",
                    "target_tokens": member,
                    "o200k_tokens": member,
                    "gating_target_tokens": member,
                    "over_ceiling_by": 0,
                    "files": [
                        file("pkg/AGENTS.md", LINT_BUDGET_MEMBER),
                        file("shared.md", LINT_BUDGET_SHARED),
                    ],
                    "outside_boundary": [],
                    "unread": [],
                },
            ],
        })
    );
}

#[test]
fn lint_context_marks_only_the_closure_that_breaches_the_ceiling() {
    let dir = lint_budget_tree();
    let root = lint_budget_tokens("o200k_base", &[LINT_BUDGET_ROOT, LINT_BUDGET_SHARED]);
    let member = lint_budget_tokens("o200k_base", &[LINT_BUDGET_MEMBER, LINT_BUDGET_SHARED]);
    assert!(member > root);
    let ceiling = root.to_string();

    let parsed = lint_json(dir.path(), &["--ceiling", &ceiling]);

    assert_eq!(parsed["budget"]["ceiling"], root);
    assert_eq!(parsed["budget"]["closures"][0]["over_ceiling_by"], 0);
    assert_eq!(
        parsed["budget"]["closures"][1]["over_ceiling_by"],
        member - root
    );

    let human = aoa()
        .current_dir(dir.path())
        .args(["lint-context", "--ceiling", &ceiling])
        .assert()
        .success();
    human
        .stdout(predicate::str::contains(format!(
            "    CLAUDE.md: {root} tokens across 2 file(s)\n"
        )))
        .stdout(predicate::str::contains(format!(
            "    pkg/AGENTS.md: {member} tokens across 2 file(s) [OVER CEILING by {}]\n",
            member - root
        )));
}

#[test]
fn lint_context_leaves_a_suppressed_file_out_of_the_breach_but_still_reports_it() {
    let dir = TempDir::new().expect("tempdir");
    let root_text = "# Root\n\nSee [big](big.md).\n";
    let big_text = format!(
        "# aoa-allow: oversized-context generated reference\n\n{}",
        "alpha beta gamma ".repeat(50)
    );
    std::fs::write(dir.path().join("AGENTS.md"), root_text).unwrap();
    std::fs::write(dir.path().join("big.md"), &big_text).unwrap();
    let root = lint_budget_tokens("o200k_base", &[root_text]);
    let big = lint_budget_tokens("o200k_base", &[&big_text]);
    let ceiling = root.to_string();

    let parsed = lint_json(dir.path(), &["--ceiling", &ceiling]);

    let closure = &parsed["budget"]["closures"][0];
    assert_eq!(closure["target_tokens"], root + big);
    assert_eq!(closure["gating_target_tokens"], root);
    assert_eq!(closure["over_ceiling_by"], 0);
    assert_eq!(closure["files"][1]["gating"], false);

    aoa()
        .current_dir(dir.path())
        .args(["lint-context", "--ceiling", &ceiling])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "      big.md: {big} tokens (suppressed, not counted against the ceiling)\n"
        )))
        .stdout(predicate::str::contains("OVER CEILING").not());
}

#[test]
fn lint_context_budget_counts_under_the_requested_tokenizer() {
    let dir = TempDir::new().expect("tempdir");
    let text = format!("# Root\n\n{}", "naïve café déjà-vu 日本語 ".repeat(40));
    std::fs::write(dir.path().join("AGENTS.md"), &text).unwrap();

    let parsed = lint_json(dir.path(), &["--tokenizer", "cl100k_base"]);

    let target = lint_budget_tokens("cl100k_base", &[&text]);
    let reference = lint_budget_tokens("o200k_base", &[&text]);
    assert_ne!(target, reference);
    assert_eq!(parsed["budget"]["tokenizer"], "cl100k_base");
    assert_eq!(parsed["budget"]["target_tokens"], target);
    assert_eq!(parsed["budget"]["o200k_tokens"], reference);
    assert_eq!(parsed["budget"]["closures"][0]["target_tokens"], target);
    assert_eq!(parsed["budget"]["closures"][0]["o200k_tokens"], reference);
}

#[test]
fn lint_context_budget_is_not_narrowed_by_changed() {
    let dir = lint_budget_tree();

    let whole = lint_json(dir.path(), &[]);
    let narrowed = lint_json(dir.path(), &["--changed", "pkg/AGENTS.md"]);

    assert_eq!(narrowed["budget"], whole["budget"]);
}

#[test]
fn lint_context_human_prints_tokens_for_the_root_closure_and_each_member() {
    let dir = lint_budget_tree();

    let root = lint_budget_tokens("o200k_base", &[LINT_BUDGET_ROOT, LINT_BUDGET_SHARED]);
    let member = lint_budget_tokens("o200k_base", &[LINT_BUDGET_MEMBER, LINT_BUDGET_SHARED]);
    let distinct = lint_budget_tokens(
        "o200k_base",
        &[LINT_BUDGET_ROOT, LINT_BUDGET_SHARED, LINT_BUDGET_MEMBER],
    );

    aoa()
        .current_dir(dir.path())
        .arg("lint-context")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "  context budget (o200k_base): {distinct} tokens across 3 file(s), ceiling 2000 per closure\n"
        )))
        .stdout(predicate::str::contains(format!(
            "    CLAUDE.md: {root} tokens across 2 file(s)\n"
        )))
        .stdout(predicate::str::contains(format!(
            "    pkg/AGENTS.md: {member} tokens across 2 file(s)\n"
        )))
        .stdout(predicate::str::contains(format!(
            "      shared.md: {} tokens\n",
            lint_budget_tokens("o200k_base", &[LINT_BUDGET_SHARED])
        )));
}

#[test]
fn lint_context_without_any_context_file_says_how_to_name_one() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("README.md"), "# Readme\n").unwrap();

    aoa()
        .current_dir(dir.path())
        .arg("lint-context")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no AGENTS.md or CLAUDE.md found"))
        .stderr(predicate::str::contains("--root"));
}

#[test]
fn lint_context_missing_explicit_root_states_the_cause_once() {
    let dir = TempDir::new().expect("tempdir");

    let output = aoa()
        .current_dir(dir.path())
        .args(["lint-context", "--root", "AGENTS.md"])
        .output()
        .expect("run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr
            .matches("failed to read context file AGENTS.md")
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(stderr.matches("os error").count(), 1, "{stderr}");
}

#[cfg(unix)]
#[test]
fn audit_reads_the_pre_commit_plane_from_the_repository_not_the_machine_git_config() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TempDir::new().expect("tempdir");
    init_git_repo(repo.path());
    let machine_hooks = repo.path().join("machine-hooks");
    std::fs::create_dir_all(&machine_hooks).unwrap();
    let hook = machine_hooks.join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let machine_config = TempDir::new().expect("tempdir");
    let global = machine_config.path().join("gitconfig");
    std::fs::write(
        &global,
        format!("[core]\n\thooksPath = {}\n", machine_hooks.display()),
    )
    .unwrap();

    let output = aoa()
        .args(["audit", "--json", "--repo"])
        .arg(repo.path())
        .env("GIT_CONFIG_GLOBAL", &global)
        .output()
        .expect("run");
    assert!(output.status.success(), "{output:?}");
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert!(
        parsed["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["plane"] == "pre-commit"),
        "a hooks path set only in the machine's git config is not this repository's plane"
    );
}
