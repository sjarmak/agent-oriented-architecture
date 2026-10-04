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

fn across_what_is_outside<T>(outside: &std::path::Path, observe: impl Fn() -> T) -> [T; 4] {
    let while_absent = observe();
    std::fs::create_dir(outside).unwrap();
    let while_a_directory = observe();
    std::fs::remove_dir(outside).unwrap();
    std::fs::write(outside, "outside\n").unwrap();
    let while_a_file = observe();
    std::fs::remove_file(outside).unwrap();
    std::os::unix::fs::symlink("removed", outside).unwrap();
    let while_a_broken_link = observe();
    [
        while_absent,
        while_a_directory,
        while_a_file,
        while_a_broken_link,
    ]
}

#[test]
fn a_context_root_linked_out_of_the_repository_fails_the_audit_whatever_it_reaches() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::os::unix::fs::symlink("../probed.md", repo.join("AGENTS.md")).unwrap();

    let observed = across_what_is_outside(&dir.path().join("probed.md"), || {
        audit(&repo, &AuditConfig::default()).map(|report| report.items.len())
    });

    for seen in observed {
        assert!(
            matches!(
                seen,
                Err(AuditError::Budget(BudgetError::OutsideBoundary { .. }))
            ),
            "{seen:?}"
        );
    }
}

#[test]
fn a_context_root_that_is_missing_or_dangles_inside_the_repository_is_not_measured() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    let while_missing = audit(&repo, &AuditConfig::default()).unwrap();
    std::os::unix::fs::symlink("removed.md", repo.join("AGENTS.md")).unwrap();
    let while_dangling = audit(&repo, &AuditConfig::default()).unwrap();

    for report in [while_missing, while_dangling] {
        assert!(report.context_outside_boundary.is_empty());
        assert!(report.context_unread.is_empty());
    }
}

#[test]
fn a_member_linked_out_of_the_repository_and_back_in_is_outside_whatever_it_steps_through() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_linking(dir.path(), "[probe](probe.md)\n");
    std::fs::write(repo.join("safe.md"), "safe\n").unwrap();
    std::os::unix::fs::symlink("../probed-dir/../repo/safe.md", repo.join("probe.md")).unwrap();

    let observed = across_what_is_outside(&dir.path().join("probed-dir"), || {
        let report = audit(&repo, &AuditConfig::default()).unwrap();
        (report.context_outside_boundary, report.context_unread)
    });

    for seen in observed {
        assert_eq!(seen, (vec![repo.join("probe.md")], Vec::new()));
    }
}

#[test]
fn a_context_root_beneath_a_directory_the_process_may_not_search_fails_the_audit() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let sealed = repo.join(".agents");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::write(sealed.join("AGENTS.md"), "rules\n").unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let searchable = std::fs::symlink_metadata(sealed.join("AGENTS.md")).is_ok();
    let cfg = AuditConfig {
        context_root: Some(PathBuf::from(".agents/AGENTS.md")),
        ..AuditConfig::default()
    };

    let audited = audit(&repo, &cfg).map(|report| report.items.len());

    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    if searchable {
        eprintln!(
            "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process searches a \
             mode-000 directory, so it cannot be denied a lookup"
        );
        return;
    }
    assert!(
        matches!(
            &audited,
            Err(AuditError::Budget(BudgetError::Io { source, .. }))
                if source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "{audited:?}"
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
