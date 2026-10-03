#![cfg(unix)]

use std::path::PathBuf;

use aoa_audit::{audit, AuditConfig, AuditError};
use aoa_budget::{BudgetError, UnreadLink, UnreadReason, MAX_CONTEXT_FILE_BYTES};

#[test]
fn a_context_root_that_resolves_outside_the_repository_fails_the_audit_unmeasured() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.md");
    std::fs::write(&secret, "outside the repository\n".repeat(64)).unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::os::unix::fs::symlink(&secret, repo.join("AGENTS.md")).unwrap();
    let cfg = AuditConfig {
        context_root: Some(PathBuf::from("AGENTS.md")),
        ceiling: 0,
        ..AuditConfig::default()
    };

    let err = audit(&repo, &cfg).unwrap_err();

    assert!(
        matches!(err, AuditError::Budget(BudgetError::OutsideBoundary { .. })),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "outside the repository\n".repeat(64)
    );
}

fn repo_linking(dir: &std::path::Path, root_text: &str) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("AGENTS.md"), root_text).unwrap();
    repo
}

#[test]
fn a_link_that_leaves_the_repository_is_named_on_the_report() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.md"), "elsewhere\n").unwrap();
    let repo = repo_linking(dir.path(), "[out](../notes.md)\n");

    let report = audit(&repo, &AuditConfig::default()).unwrap();

    assert_eq!(
        report.context_outside_boundary,
        [dir.path().join("notes.md")]
    );
    assert!(report.render_human().contains(&format!(
        "context link not counted, it resolves outside the repository: {}",
        dir.path().join("notes.md").display()
    )));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["context_outside_boundary"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn a_member_that_cannot_be_counted_is_named_on_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_linking(dir.path(), "[binary](binary.md) [gone](gone.md)\n");
    std::fs::write(repo.join("binary.md"), b"rules\n\xff\n").unwrap();
    std::os::unix::fs::symlink("removed.md", repo.join("gone.md")).unwrap();

    let report = audit(&repo, &AuditConfig::default()).unwrap();

    assert_eq!(
        report.context_unread,
        [
            UnreadLink {
                path: repo.join("binary.md"),
                reason: UnreadReason::NotUtf8,
            },
            UnreadLink {
                path: repo.join("gone.md"),
                reason: UnreadReason::BrokenSymlink,
            },
        ]
    );
    let human = report.render_human();
    assert!(
        human.contains("context link not counted, it is not UTF-8 text:"),
        "{human}"
    );
}

#[test]
fn a_report_with_every_link_counted_omits_the_uncounted_fields() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_linking(dir.path(), "rules\n");

    let json = serde_json::to_value(audit(&repo, &AuditConfig::default()).unwrap()).unwrap();

    assert!(json.get("context_outside_boundary").is_none());
    assert!(json.get("context_unread").is_none());
}

#[test]
fn an_oversized_member_fails_the_audit_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_linking(dir.path(), "[big](big.md)\n");
    let big = std::fs::File::create(repo.join("big.md")).unwrap();
    big.set_len(MAX_CONTEXT_FILE_BYTES + 1).unwrap();

    let err = audit(&repo, &AuditConfig::default()).unwrap_err();

    assert!(err.to_string().contains("big.md"), "{err}");
}
