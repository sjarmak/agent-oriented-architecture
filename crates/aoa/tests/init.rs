//! Integration tests for `aoa init` (R11): the ejectable agent-ready scaffold.
//!
//! Acceptance criteria under test:
//! - a fresh init produces the expected plain-text file set (no runtime lock-in),
//! - `rm -rf .aoa/` leaves a working repo (no generated file references `.aoa/`),
//! - `init --update` migrates a generated repo with a reviewable diff artifact
//!   (`<path>.new`) instead of silently overwriting user edits,
//! - the generated AGENTS.md closure passes the lint-context detectors with zero
//!   findings and the audit context-token ceiling (2000 tokens).

use std::path::PathBuf;
use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use tempfile::TempDir;

fn aoa() -> Command {
    Command::cargo_bin("aoa").expect("aoa binary builds")
}

/// The relative paths a fresh init writes (excluding the manifest).
const SCAFFOLD_FILES: &[&str] = &[
    "AGENTS.md",
    "aoa-policy.yaml",
    "features/README.md",
    ".claude/skills/feature-capsule/SKILL.md",
];

const MANIFEST: &str = ".aoa/init.json";

fn init_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    aoa()
        .args(["init", "--repo"])
        .arg(dir.path())
        .assert()
        .success();
    dir
}

#[test]
fn fresh_init_writes_expected_plain_text_files() {
    let dir = init_repo();
    for rel in SCAFFOLD_FILES {
        let path = dir.path().join(rel);
        assert!(path.is_file(), "missing scaffold file {rel}");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{rel} is not plain UTF-8 text: {e}"));
        assert!(!text.is_empty(), "{rel} is empty");
    }
    assert!(dir.path().join(MANIFEST).is_file(), "manifest missing");

    // The project name is substituted, no template markers leak.
    for rel in SCAFFOLD_FILES {
        let text = std::fs::read_to_string(dir.path().join(rel)).unwrap();
        assert!(
            !text.contains("{{"),
            "unrendered template marker left in {rel}"
        );
    }
}

#[test]
fn second_init_refuses_and_points_at_update() {
    let dir = init_repo();
    aoa()
        .args(["init", "--repo"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--update"));
}

#[test]
fn init_never_overwrites_an_existing_file() {
    let dir = TempDir::new().expect("tempdir");
    let agents = dir.path().join("AGENTS.md");
    std::fs::write(&agents, "user content\n").unwrap();

    aoa()
        .args(["init", "--repo"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("skipped"));

    assert_eq!(std::fs::read_to_string(&agents).unwrap(), "user content\n");
    // The rest of the scaffold is still written.
    assert!(dir.path().join("aoa-policy.yaml").is_file());
}

#[test]
fn eject_leaves_a_self_sufficient_repo() {
    let dir = init_repo();
    std::fs::remove_dir_all(dir.path().join(".aoa")).unwrap();

    // Every scaffold file survives and none references the ejected .aoa/ tree.
    for rel in SCAFFOLD_FILES {
        let path = dir.path().join(rel);
        assert!(path.is_file(), "{rel} lost on eject");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(".aoa"), "{rel} references the ejected .aoa/");
    }

    // The scaffold still lints clean without the manifest.
    aoa()
        .args(["lint-context", "--root"])
        .arg(dir.path().join("AGENTS.md"))
        .assert()
        .success()
        .stdout(predicate::str::contains("0 finding(s)"));
}

#[test]
fn update_on_pristine_repo_is_a_clean_no_op() {
    let dir = init_repo();
    let before = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();

    aoa()
        .args(["init", "--update", "--repo"])
        .arg(dir.path())
        .assert()
        .success();

    assert_eq!(
        std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
        before
    );
    assert!(
        !dir.path().join("AGENTS.md.new").exists(),
        "no .new artifact on a pristine update"
    );
}

#[test]
fn update_preserves_user_edits_and_writes_review_artifact() {
    let dir = init_repo();
    let agents = dir.path().join("AGENTS.md");
    std::fs::write(&agents, "locally customized\n").unwrap();

    aoa()
        .args(["init", "--update", "--repo"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("AGENTS.md.new"));

    // User content untouched; the new render lands beside it for review.
    assert_eq!(
        std::fs::read_to_string(&agents).unwrap(),
        "locally customized\n"
    );
    let proposed = std::fs::read_to_string(dir.path().join("AGENTS.md.new")).unwrap();
    assert!(proposed.contains("# "), "review artifact holds the render");
}

#[test]
fn update_restores_a_deleted_scaffold_file() {
    let dir = init_repo();
    std::fs::remove_file(dir.path().join("features/README.md")).unwrap();

    aoa()
        .args(["init", "--update", "--repo"])
        .arg(dir.path())
        .assert()
        .success();

    assert!(dir.path().join("features/README.md").is_file());
}

#[test]
fn update_without_manifest_fails_loud() {
    let dir = TempDir::new().expect("tempdir");
    aoa()
        .args(["init", "--update", "--repo"])
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("aoa init"));
}

#[test]
fn generated_agents_md_passes_lint_context_and_token_ceiling() {
    let dir = init_repo();

    // Zero findings from the mechanical detectors over the whole closure.
    aoa()
        .args(["lint-context", "--root"])
        .arg(dir.path().join("AGENTS.md"))
        .assert()
        .success()
        .stdout(predicate::str::contains("0 finding(s)"));

    // The closure passes the audit context ceiling (2000 target tokens —
    // crates/aoa-audit DEFAULT_CONTEXT_CEILING) as a blocking budget.
    let closure = aoa_budget::resolve_closure(&dir.path().join("AGENTS.md")).expect("closure");
    let report =
        aoa_budget::count_budget(&closure, "o200k_base", &aoa_budget::Config::blocking(2_000))
            .expect("budget");
    assert_eq!(
        report.verdict,
        aoa_budget::Verdict::Pass,
        "AGENTS.md closure is {} tokens, over the 2000-token audit ceiling",
        report.gating_target_tokens
    );
}

#[test]
fn starter_policy_compiles_through_the_policy_pipeline() {
    let dir = init_repo();
    aoa()
        .args(["policy", "compile", "--repo"])
        .arg(dir.path())
        .assert()
        .success();
}

#[test]
fn init_json_output_lists_written_files() {
    let dir = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["init", "--json", "--repo"])
        .arg(dir.path())
        .output()
        .expect("run");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    let written: Vec<&str> = parsed["written"]
        .as_array()
        .expect("written array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for rel in SCAFFOLD_FILES {
        assert!(written.contains(rel), "{rel} not reported as written");
    }
}

#[test]
fn manifest_records_version_and_relative_paths_with_hashes() {
    let dir = init_repo();
    let raw = std::fs::read_to_string(dir.path().join(MANIFEST)).expect("manifest readable");
    let manifest: serde_json::Value = serde_json::from_str(&raw).expect("manifest is valid JSON");
    assert!(manifest["template_version"].is_number());
    let files = manifest["files"].as_array().expect("files array");
    assert_eq!(files.len(), SCAFFOLD_FILES.len());
    for entry in files {
        let path = PathBuf::from(entry["path"].as_str().expect("path"));
        assert!(path.is_relative(), "manifest path must be relative");
        let hash = entry["sha256"].as_str().expect("sha256");
        assert_eq!(hash.len(), 64, "sha256 hex digest expected");
    }
}
