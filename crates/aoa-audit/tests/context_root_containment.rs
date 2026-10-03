#![cfg(unix)]

use std::path::PathBuf;

use aoa_audit::{audit, AuditConfig, AuditError};
use aoa_budget::BudgetError;

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
