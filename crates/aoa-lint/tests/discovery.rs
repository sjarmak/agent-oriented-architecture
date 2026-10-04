use std::fs;
use std::path::{Path, PathBuf};

use aoa_lint::{
    discover_context_roots, lint_context_roots, ClosureBudget, LintError, SmellCategory,
};
use tempfile::TempDir;

const DUPLICATED_HEADING: &str = "# Rules\n\nfirst\n\n# Rules\n\nsecond\n";

fn write(base: &Path, relative: &str, text: &str) -> PathBuf {
    let path = base.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(&path, text).expect("write file");
    path
}

fn relative_roots(base: &Path) -> Vec<PathBuf> {
    discover_context_roots(base)
        .expect("discovery succeeds")
        .into_iter()
        .map(|root| root.strip_prefix(base).expect("under base").to_path_buf())
        .collect()
}

#[test]
fn discovers_every_nested_agents_and_claude_file_in_path_order() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "CLAUDE.md", "# Root\n");
    write(dir.path(), "services/api/CLAUDE.md", "# Api\n");
    write(dir.path(), "services/web/AGENTS.md", "# Web\n");
    write(dir.path(), ".claude/CLAUDE.md", "# Hidden\n");
    write(dir.path(), "services/web/README.md", "# Not a root\n");

    assert_eq!(
        relative_roots(dir.path()),
        [
            PathBuf::from(".claude/CLAUDE.md"),
            PathBuf::from("CLAUDE.md"),
            PathBuf::from("services/api/CLAUDE.md"),
            PathBuf::from("services/web/AGENTS.md"),
        ]
    );
}

#[test]
fn discovery_skips_git_metadata_and_gitignored_directories() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), ".gitignore", "vendor/\n");
    write(dir.path(), "AGENTS.md", "# Root\n");
    write(dir.path(), "vendor/pkg/CLAUDE.md", "# Vendored\n");
    write(dir.path(), ".git/CLAUDE.md", "# Metadata\n");

    assert_eq!(relative_roots(dir.path()), [PathBuf::from("AGENTS.md")]);
}

#[test]
fn discovery_of_a_missing_directory_fails() {
    let dir = TempDir::new().expect("tempdir");
    let result = discover_context_roots(&dir.path().join("absent"));
    assert!(matches!(result, Err(LintError::Walk { .. })));
}

#[test]
fn every_root_is_linted_and_a_shared_file_is_linted_once() {
    let dir = TempDir::new().expect("tempdir");
    let root = write(
        dir.path(),
        "CLAUDE.md",
        "# Root\n\nSee [shared](shared.md).\n",
    );
    let nested = write(
        dir.path(),
        "pkg/CLAUDE.md",
        &format!("{DUPLICATED_HEADING}\nSee [shared](../shared.md).\n"),
    );
    write(dir.path(), "shared.md", DUPLICATED_HEADING);

    let report =
        lint_context_roots(&[root, nested], dir.path(), "o200k_base").expect("lint succeeds");

    let duplicated: Vec<PathBuf> = report
        .findings
        .iter()
        .filter(|finding| finding.category == SmellCategory::Duplication)
        .map(|finding| {
            finding
                .file
                .strip_prefix(dir.path())
                .expect("under base")
                .to_path_buf()
        })
        .collect();
    assert_eq!(
        duplicated,
        [PathBuf::from("shared.md"), PathBuf::from("pkg/CLAUDE.md")]
    );
    assert_eq!(report.budget.files.len(), 3);
}

#[test]
fn each_root_reports_its_own_closure_tokens_and_a_shared_file_counts_in_both() {
    let dir = TempDir::new().expect("tempdir");
    let root = write(
        dir.path(),
        "CLAUDE.md",
        "# Root\n\nSee [shared](shared.md).\n",
    );
    let nested = write(
        dir.path(),
        "pkg/CLAUDE.md",
        "# Member\n\nSee [shared](../shared.md) and nothing else at all.\n",
    );
    let shared = write(dir.path(), "shared.md", "# Shared\n\nalpha beta gamma\n");

    let report = lint_context_roots(&[root.clone(), nested.clone()], dir.path(), "o200k_base")
        .expect("lint succeeds");

    let counted = |path: &PathBuf| {
        report
            .budget
            .files
            .iter()
            .find(|file| &file.path == path)
            .expect("file counted")
            .clone()
    };
    let tokens = |path: &PathBuf| counted(path).target_tokens;
    assert_eq!(
        report.closures,
        [
            ClosureBudget {
                root: root.clone(),
                o200k_tokens: tokens(&root) + tokens(&shared),
                target_tokens: tokens(&root) + tokens(&shared),
                gating_target_tokens: tokens(&root) + tokens(&shared),
                files: vec![counted(&root), counted(&shared)],
                outside_boundary: Vec::new(),
                unread: Vec::new(),
            },
            ClosureBudget {
                root: nested.clone(),
                o200k_tokens: tokens(&nested) + tokens(&shared),
                target_tokens: tokens(&nested) + tokens(&shared),
                gating_target_tokens: tokens(&nested) + tokens(&shared),
                files: vec![counted(&nested), counted(&shared)],
                outside_boundary: Vec::new(),
                unread: Vec::new(),
            },
        ]
    );
    assert!(tokens(&root) > 0 && tokens(&nested) > 0 && tokens(&shared) > 0);
    assert_eq!(
        report.budget.target_tokens,
        tokens(&root) + tokens(&nested) + tokens(&shared)
    );
}

#[test]
fn a_suppressed_file_is_reported_but_left_out_of_the_closure_gating_tokens() {
    let dir = TempDir::new().expect("tempdir");
    let root = write(dir.path(), "AGENTS.md", "# Root\n\nSee [big](big.md).\n");
    write(
        dir.path(),
        "big.md",
        "# aoa-allow: oversized-context generated reference\n\n# Big\n\nalpha beta\n",
    );

    let report =
        lint_context_roots(std::slice::from_ref(&root), dir.path(), "o200k_base").expect("lint");

    let closure = &report.closures[0];
    let [root_file, big] = closure.files.as_slice() else {
        panic!("expected two files, got {:?}", closure.files);
    };
    assert!(root_file.gating);
    assert!(!big.gating);
    assert_eq!(
        closure.target_tokens,
        root_file.target_tokens + big.target_tokens
    );
    assert_eq!(closure.gating_target_tokens, root_file.target_tokens);
}

#[test]
fn linting_no_roots_fails() {
    let result = lint_context_roots(&[], Path::new("."), "o200k_base");
    assert!(matches!(result, Err(LintError::NoRoots)));
}

#[test]
fn ignore_file_above_the_linted_directory_does_not_hide_context_roots() {
    let outer = TempDir::new().expect("tempdir");
    write(outer.path(), ".gitignore", "CLAUDE.md\nservices/\n");
    let repo = outer.path().join("snapshot");
    write(&repo, "CLAUDE.md", "# Root\n");
    write(&repo, "services/api/AGENTS.md", "# Api\n");

    assert_eq!(
        relative_roots(&repo),
        [
            PathBuf::from("CLAUDE.md"),
            PathBuf::from("services/api/AGENTS.md"),
        ]
    );
}

#[test]
fn ignore_line_the_matcher_cannot_apply_fails_discovery_and_names_the_file() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "services/.gitignore", "{foo\n");
    write(dir.path(), "services/api/AGENTS.md", "# Api\n");

    let err = discover_context_roots(dir.path()).expect_err("unapplied ignore rule");

    assert!(matches!(err, LintError::Walk { .. }), "{err}");
    let source = std::error::Error::source(&err).expect("source").to_string();
    assert!(source.contains(".gitignore"), "{source}");
}

#[test]
fn ignore_file_that_cannot_be_read_fails_discovery_and_names_the_file() {
    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        write(dir.path(), "services/api/AGENTS.md", "# Api\n");
        fs::create_dir(dir.path().join("services").join(name)).expect("ignore dir");

        let err = discover_context_roots(dir.path()).expect_err("unreadable ignore file");

        assert!(
            matches!(&err, LintError::Walk { dir, .. } if dir.ends_with(name)),
            "{err}"
        );
    }
}

#[cfg(unix)]
#[test]
fn unreadable_ignore_file_under_a_symlinked_root_fails_discovery_and_names_the_file() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "repo/AGENTS.md", "# Root\n");
    fs::create_dir(dir.path().join("repo/.gitignore")).expect("ignore dir");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("repo"), &link).expect("symlink");

    let err = discover_context_roots(&link).expect_err("unreadable ignore file");

    assert!(
        matches!(&err, LintError::Walk { dir, .. } if dir.ends_with(".gitignore")),
        "{err}"
    );
}

#[test]
fn ignore_file_that_is_not_text_fails_discovery_and_names_the_file() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "services/api/AGENTS.md", "# Api\n");
    fs::write(dir.path().join("services/.gitignore"), b"api/\n\xff\xfe\n").expect("write ignore");

    let err = discover_context_roots(dir.path()).expect_err("ignore file is not text");

    assert!(
        matches!(&err, LintError::Walk { dir, .. } if dir.ends_with(".gitignore")),
        "{err}"
    );
}

#[cfg(unix)]
fn leaving_link_error(base: &Path, ignore_file: &str) -> String {
    match discover_context_roots(&base.join("repo")) {
        Err(LintError::IgnoreFileOutside { path, .. }) if path.ends_with(ignore_file) => path
            .strip_prefix(base)
            .expect("under base")
            .display()
            .to_string(),
        other => panic!("expected a refused ignore file link, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn ignore_file_link_leaving_the_linted_directory_is_refused_whatever_it_reaches() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let dir = TempDir::new().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical");
    write(&base, "repo/AGENTS.md", "# Root\n");
    write(&base, "repo/services/api/AGENTS.md", "# Api\n");
    symlink("../probed", base.join("repo/.gitignore")).expect("direct link");
    let probed = base.join("probed");

    let while_absent = leaving_link_error(&base, ".gitignore");
    fs::write(&probed, "AGENTS.md\n").expect("write outside file");
    let while_a_file = leaving_link_error(&base, ".gitignore");
    fs::set_permissions(&probed, fs::Permissions::from_mode(0o000)).expect("unreadable");
    let while_unreadable = leaving_link_error(&base, ".gitignore");
    fs::remove_file(&probed).expect("remove outside file");
    fs::create_dir(&probed).expect("create outside directory");
    let while_a_directory = leaving_link_error(&base, ".gitignore");

    assert_eq!(while_absent, "repo/.gitignore");
    assert_eq!(while_absent, while_a_file);
    assert_eq!(while_absent, while_unreadable);
    assert_eq!(while_absent, while_a_directory);
}

#[cfg(unix)]
#[test]
fn ignore_file_link_leaving_through_a_chain_a_subdirectory_or_an_absolute_target_is_refused() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical");
    fs::write(base.join("probed"), "AGENTS.md\n").expect("write outside file");

    write(&base, "chained/repo/AGENTS.md", "# Root\n");
    symlink("hop", base.join("chained/repo/.ignore")).expect("first hop");
    symlink("../probed", base.join("chained/repo/hop")).expect("second hop");
    assert_eq!(
        leaving_link_error(&base.join("chained"), ".ignore"),
        "repo/.ignore"
    );

    write(&base, "nested/repo/services/AGENTS.md", "# Services\n");
    symlink(
        "../../../probed",
        base.join("nested/repo/services/.gitignore"),
    )
    .expect("link from a subdirectory");
    assert_eq!(
        leaving_link_error(&base.join("nested"), ".gitignore"),
        "repo/services/.gitignore"
    );

    write(&base, "absolute/repo/AGENTS.md", "# Root\n");
    symlink(base.join("probed"), base.join("absolute/repo/.gitignore")).expect("absolute link");
    assert_eq!(
        leaving_link_error(&base.join("absolute"), ".gitignore"),
        "repo/.gitignore"
    );
}

#[cfg(unix)]
#[test]
fn ignore_file_link_that_stays_inside_the_linted_directory_still_applies() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "AGENTS.md", "# Root\n");
    write(dir.path(), "rules/shared", "vendor/\n");
    write(dir.path(), "services/vendor/CLAUDE.md", "# Vendored\n");
    write(dir.path(), "services/api/AGENTS.md", "# Api\n");
    std::os::unix::fs::symlink("../rules/shared", dir.path().join("services/.gitignore"))
        .expect("link inside the directory");

    assert_eq!(
        relative_roots(dir.path()),
        [
            PathBuf::from("AGENTS.md"),
            PathBuf::from("services/api/AGENTS.md"),
        ]
    );
}

#[cfg(unix)]
fn leaving_link_error_within_five_seconds(base: &Path, ignore_file: &'static str) -> String {
    let (finished, outcome) = std::sync::mpsc::channel();
    let base = base.to_path_buf();
    std::thread::spawn(move || finished.send(leaving_link_error(&base, ignore_file)));
    outcome
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("discovery refuses the link without reading through it")
}

#[cfg(unix)]
#[test]
fn ignore_file_link_leaving_the_linted_directory_is_refused_without_being_read_through() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical");
    let made = std::process::Command::new("mkfifo")
        .arg(base.join("probed"))
        .status()
        .expect("run mkfifo");
    assert!(made.success());

    write(&base, "direct/repo/AGENTS.md", "# Root\n");
    symlink("../../probed", base.join("direct/repo/.gitignore")).expect("direct link");
    assert_eq!(
        leaving_link_error_within_five_seconds(&base.join("direct"), ".gitignore"),
        "repo/.gitignore"
    );

    write(&base, "nested/repo/services/AGENTS.md", "# Services\n");
    symlink("../../../probed", base.join("nested/repo/services/.ignore"))
        .expect("link from a subdirectory");
    assert_eq!(
        leaving_link_error_within_five_seconds(&base.join("nested"), ".ignore"),
        "repo/services/.ignore"
    );
}
