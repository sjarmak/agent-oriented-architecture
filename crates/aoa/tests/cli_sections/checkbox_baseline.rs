use super::*;

// --- aoa gap checkbox-baseline (aoa-d6t.28) ---------------------------------

/// A fixture tree passing all four level-1 mechanical criteria, so the
/// checkbox baseline lands at level 2 (Documented).
fn checkbox_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let p = dir.path();
    std::fs::write(p.join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(p.join("README.md"), "# demo\n").unwrap();
    std::fs::write(p.join("clippy.toml"), "").unwrap();
    std::fs::create_dir(p.join("tests")).unwrap();
    dir
}

// The JSON register emits the full CheckboxBaseline artifact — repo id, pinned
// criteria version, achieved level, and every criterion including excluded
// ones with their reasons — so study repos can be scored in batch.
#[test]
fn gap_checkbox_baseline_json_emits_full_artifact() {
    let repo = checkbox_repo();
    let output = aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo", "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["repo_id"], "demo-repo");
    assert_eq!(parsed["level"], 2);
    assert!(parsed["criteria_version"]
        .as_str()
        .unwrap()
        .starts_with("factory-provisional-"));
    let criteria = parsed["criteria"].as_array().expect("criteria array");
    let excluded: Vec<&Value> = criteria
        .iter()
        .filter(|c| c["status"]["status"] == "excluded")
        .collect();
    assert!(
        !excluded.is_empty(),
        "excluded criteria are carried in JSON, never dropped"
    );
    for c in &excluded {
        assert!(
            !c["status"]["reason"].as_str().unwrap().is_empty(),
            "every excluded criterion carries its reason"
        );
    }
}

// Default human register: achieved level with its Factory name, per-level and
// per-pillar tallies, and the excluded COUNT — but not the reason prose.
#[test]
fn gap_checkbox_baseline_human_summarizes_levels_and_pillars() {
    let repo = checkbox_repo();
    let assert = aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo-repo"))
        .stdout(predicate::str::contains("level 2 (Documented)"))
        .stdout(predicate::str::contains("style_and_validation"));
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    assert!(
        !stdout.contains("hosting-platform setting"),
        "exclusion reasons stay behind --show-excluded"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "no ANSI escapes when stdout is not a terminal"
    );
}

// --show-excluded surfaces every excluded criterion with its reason in the
// human register; the JSON register carries them regardless (identical
// findings across registers).
#[test]
fn gap_checkbox_baseline_show_excluded_lists_reasons() {
    let repo = checkbox_repo();
    aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo", "--show-excluded"])
        .assert()
        .success()
        .stdout(predicate::str::contains("branch_protection"))
        .stdout(predicate::str::contains("hosting-platform setting"))
        .stdout(predicate::str::contains("secret_scanning"))
        .stdout(predicate::str::contains("self_improving_orchestration"));
}

// A missing root fails loud with a typed error, never a level-1 default.
#[test]
fn gap_checkbox_baseline_missing_root_fails_loud() {
    let repo = TempDir::new().expect("tempdir");
    let missing = repo.path().join("nope");
    aoa()
        .args(["gap", "checkbox-baseline"])
        .arg(&missing)
        .args(["--repo-id", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a directory"));
}

// `--json` is global on `aoa gap`: placed before the subcommand (the position
// that works for bare `aoa gap --json`) it must still select the JSON register,
// never parse-and-ignore into human text.
#[test]
fn gap_parent_json_flag_reaches_checkbox_baseline() {
    let repo = checkbox_repo();
    let output = aoa()
        .args(["gap", "--json", "checkbox-baseline"])
        .arg(repo.path())
        .args(["--repo-id", "demo-repo"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .expect("parent-position --json emits the CheckboxBaseline artifact");
    assert_eq!(parsed["repo_id"], "demo-repo");
}

// The bare `aoa gap` surface (no subcommand) still renders the R9c
// determination — the new subcommand must not displace it.
#[test]
fn gap_without_subcommand_still_renders_determination() {
    aoa()
        .args(["gap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("construct validity"));
}
