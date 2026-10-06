use std::fs;
use std::path::{Path, PathBuf};

use aoa_lint::{
    ClosureBudget, DiscoveredRoot, LintError, LintReport, LintedDirectory, SmellCategory,
};
use tempfile::TempDir;

const DUPLICATED_HEADING: &str = "# Rules\n\nfirst\n\n# Rules\n\nsecond\n";

fn write(base: &Path, relative: &str, text: &str) -> PathBuf {
    let path = base.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(&path, text).expect("write file");
    path
}

fn held(dir: &Path) -> LintedDirectory {
    LintedDirectory::hold(dir).expect("hold the linted directory")
}

fn discover_context_roots(dir: &Path) -> Result<Vec<PathBuf>, LintError> {
    let roots = LintedDirectory::hold(dir)?.discover()?;
    Ok(paths_of(&roots))
}

fn paths_of(roots: &[DiscoveredRoot]) -> Vec<PathBuf> {
    roots.iter().map(|root| root.path().to_path_buf()).collect()
}

fn lint_discovered(dir: &Path) -> (Vec<PathBuf>, LintReport) {
    let linted = held(dir);
    let discovered = linted.discover().expect("discovery succeeds");
    let report = linted
        .lint(&[], &discovered, "o200k_base")
        .expect("lint succeeds");
    (paths_of(&discovered), report)
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

    let (discovered, report) = lint_discovered(dir.path());
    assert_eq!(discovered, [root, nested]);

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

    let (discovered, report) = lint_discovered(dir.path());
    assert_eq!(discovered, [root.clone(), nested.clone()]);

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

    let (discovered, report) = lint_discovered(dir.path());
    assert_eq!(discovered, [root]);

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
    let result = held(Path::new(".")).lint(&[], &[], "o200k_base");
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
    make_fifo(&base.join("probed"));

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

#[cfg(unix)]
fn roots_within_five_seconds(base: &Path) -> Vec<PathBuf> {
    let (finished, outcome) = std::sync::mpsc::channel();
    let base = base.to_path_buf();
    std::thread::spawn(move || finished.send(relative_roots(&base.join("repo"))));
    outcome
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("discovery finishes without opening anything above the linted directory")
}

#[cfg(unix)]
#[test]
fn ignore_files_above_the_linted_directory_are_never_opened() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        write(&base, "repo/AGENTS.md", "# Root\n");
        write(&base, "repo/.gitignore", "vendor/\n");
        write(&base, "repo/vendor/CLAUDE.md", "# Vendored\n");
        let above = base.join(name);
        let expected = [PathBuf::from("AGENTS.md")];

        assert_eq!(roots_within_five_seconds(&base), expected, "{name} absent");

        make_fifo(&base.join("outside"));
        symlink("outside", &above).expect("link to a fifo");
        assert_eq!(roots_within_five_seconds(&base), expected, "{name} fifo");

        fs::remove_file(&above).expect("remove link");
        symlink("/dev/zero", &above).expect("link to an endless device");
        assert_eq!(roots_within_five_seconds(&base), expected, "{name} device");

        fs::remove_file(&above).expect("remove link");
        fs::write(&above, "AGENTS.md\n").expect("write ignore file above");
        assert_eq!(roots_within_five_seconds(&base), expected, "{name} file");

        fs::set_permissions(&above, fs::Permissions::from_mode(0o000)).expect("unreadable");
        assert_eq!(
            roots_within_five_seconds(&base),
            expected,
            "{name} unreadable"
        );
    }
}

#[test]
fn ignore_files_inside_the_linted_directory_apply_with_dot_ignore_overriding_gitignore() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "AGENTS.md", "# Root\n");
    write(dir.path(), ".gitignore", "vendor/\nkept/\n");
    write(dir.path(), ".ignore", "!kept/\n");
    write(dir.path(), "vendor/CLAUDE.md", "# Vendored\n");
    write(dir.path(), "kept/CLAUDE.md", "# Kept\n");
    write(dir.path(), "services/.ignore", "generated/\n");
    write(dir.path(), "services/generated/AGENTS.md", "# Generated\n");
    write(dir.path(), "services/api/AGENTS.md", "# Api\n");

    assert_eq!(
        relative_roots(dir.path()),
        [
            PathBuf::from("AGENTS.md"),
            PathBuf::from("kept/CLAUDE.md"),
            PathBuf::from("services/api/AGENTS.md"),
        ]
    );
}

#[cfg(unix)]
fn special_file_error_within_five_seconds(base: &Path, ignore_file: &'static str) -> String {
    let (finished, outcome) = std::sync::mpsc::channel();
    let base = base.to_path_buf();
    std::thread::spawn(move || {
        let refused = match discover_context_roots(&base.join("repo")) {
            Err(LintError::IgnoreFileNotRegular { path }) if path.ends_with(ignore_file) => path
                .strip_prefix(&base)
                .expect("under base")
                .display()
                .to_string(),
            other => format!("expected a refused ignore file, got {other:?}"),
        };
        finished.send(refused)
    });
    outcome
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("discovery refuses the ignore file without opening it")
}

#[cfg(unix)]
fn make_fifo(path: &Path) {
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("make a fifo");
}

#[cfg(unix)]
#[test]
fn ignore_file_that_is_a_fifo_inside_the_linted_directory_is_refused_without_being_opened() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical");

    for name in [".gitignore", ".ignore"] {
        let case = base.join(format!("root{name}"));
        write(&case, "repo/AGENTS.md", "# Root\n");
        make_fifo(&case.join("repo").join(name));
        assert_eq!(
            special_file_error_within_five_seconds(&case, name),
            format!("repo/{name}")
        );
    }

    let nested = base.join("nested");
    write(&nested, "repo/services/AGENTS.md", "# Services\n");
    make_fifo(&nested.join("repo/services/.gitignore"));
    assert_eq!(
        special_file_error_within_five_seconds(&nested, ".gitignore"),
        "repo/services/.gitignore"
    );

    let linked = base.join("linked");
    write(&linked, "repo/AGENTS.md", "# Root\n");
    make_fifo(&linked.join("repo/pipe"));
    symlink("pipe", linked.join("repo/.ignore")).expect("link to a fifo inside");
    assert_eq!(
        special_file_error_within_five_seconds(&linked, ".ignore"),
        "repo/.ignore"
    );
}

#[cfg(unix)]
#[test]
fn ignore_file_with_a_second_name_outside_the_linted_directory_is_refused_and_left_alone() {
    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        write(&base, "repo/AGENTS.md", "# Root\n");
        write(&base, "repo/services/AGENTS.md", "# Services\n");
        let outside = write(&base, "outside", "AGENTS.md\n");

        for holder in ["repo", "repo/services"] {
            let planted = base.join(holder).join(name);
            fs::hard_link(&outside, &planted).expect("hard link the ignore file");

            let refused = discover_context_roots(&base.join("repo"));

            assert!(
                matches!(
                    &refused,
                    Err(LintError::IgnoreFileHardLinked { path }) if path == &planted
                ),
                "{holder}/{name}: {refused:?}"
            );
            assert_eq!(
                fs::read_to_string(&outside).expect("read outside file"),
                "AGENTS.md\n"
            );
            fs::remove_file(&planted).expect("remove the hard link");
        }
        assert_eq!(
            relative_roots(&base.join("repo")),
            [
                PathBuf::from("AGENTS.md"),
                PathBuf::from("services/AGENTS.md"),
            ]
        );
    }
}

#[cfg(unix)]
#[test]
fn ignore_file_caught_in_a_link_loop_fails_discovery_and_names_the_file() {
    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        write(&base, "repo/AGENTS.md", "# Root\n");
        let looping = base.join("repo").join(name);
        std::os::unix::fs::symlink(name, &looping).expect("link the ignore file to itself");

        let refused = discover_context_roots(&base.join("repo"));

        assert!(
            matches!(&refused, Err(LintError::Walk { dir, .. }) if dir == &looping),
            "{name}: {refused:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_discovered_root_whose_directory_became_a_link_is_read_as_held_while_the_same_root_named_follows_the_link(
) {
    let dir = TempDir::new().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical");
    let repo = base.join("repo");
    write(&repo, "AGENTS.md", "# Root\n");
    write(&repo, "docs/CLAUDE.md", "# Docs\n");
    let outside = write(&base, "outside/CLAUDE.md", DUPLICATED_HEADING);
    let linted = held(&repo);
    let discovered = linted.discover().expect("discovery succeeds");
    assert_eq!(
        paths_of(&discovered),
        [repo.join("AGENTS.md"), repo.join("docs/CLAUDE.md")]
    );
    fs::rename(repo.join("docs"), base.join("elsewhere")).expect("move docs aside");
    std::os::unix::fs::symlink(base.join("outside"), repo.join("docs")).expect("swap docs");
    assert_eq!(
        fs::read_to_string(repo.join("docs/CLAUDE.md")).expect("the link reaches the outside file"),
        DUPLICATED_HEADING
    );

    let as_discovered = linted
        .lint(&[], &discovered, "o200k_base")
        .expect("discovered roots are read as held");
    let as_named = linted
        .lint(&[repo.join("docs/CLAUDE.md")], &[], "o200k_base")
        .expect("a root the caller named is read as named");

    assert_eq!(as_discovered.closures[1].root, repo.join("docs/CLAUDE.md"));
    assert_eq!(as_discovered.budget.files.len(), 2);
    assert!(
        as_discovered.findings.is_empty(),
        "outside content was linted: {:?}",
        as_discovered.findings
    );
    assert_eq!(as_named.closures[0].root, repo.join("docs/CLAUDE.md"));
    assert!(as_named
        .findings
        .iter()
        .any(|finding| finding.category == SmellCategory::Duplication));
    assert_eq!(
        fs::read_to_string(&outside).expect("outside file untouched"),
        DUPLICATED_HEADING
    );
}

#[cfg(unix)]
#[test]
fn the_linted_directory_swapped_for_a_link_between_discovery_and_reading_is_read_as_discovered() {
    let dir = TempDir::new().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical");
    let repo = base.join("repo");
    write(&repo, "AGENTS.md", "# Root\n\nSee [shared](shared.md).\n");
    write(&repo, "shared.md", "# Shared\n");
    write(
        &base,
        "outside/AGENTS.md",
        &format!("{DUPLICATED_HEADING}\nSee [shared](shared.md).\n"),
    );
    write(&base, "outside/shared.md", DUPLICATED_HEADING);
    let linted = held(&repo);
    let discovered = linted.discover().expect("discovery succeeds");
    assert_eq!(paths_of(&discovered), [repo.join("AGENTS.md")]);
    fs::rename(&repo, base.join("elsewhere")).expect("move the linted directory aside");
    std::os::unix::fs::symlink(base.join("outside"), &repo).expect("swap it for a link");
    assert!(fs::read_to_string(repo.join("AGENTS.md"))
        .expect("the link reaches outside")
        .contains("second"));

    let report = linted
        .lint(&[], &discovered, "o200k_base")
        .expect("lint succeeds");

    assert!(
        report.findings.is_empty(),
        "outside content was linted: {:?}",
        report.findings
    );
    assert_eq!(report.closures[0].root, repo.join("AGENTS.md"));
    assert_eq!(
        paths_of_files(&report),
        [repo.join("AGENTS.md"), repo.join("shared.md")]
    );
    assert_eq!(report.closures[0].outside_boundary, Vec::<PathBuf>::new());
    assert_eq!(report.closures[0].unread, Vec::new());
}

fn paths_of_files(report: &LintReport) -> Vec<PathBuf> {
    report
        .budget
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect()
}

#[test]
fn an_ignore_file_larger_than_a_mebibyte_is_refused_and_one_exactly_that_size_is_read() {
    let base = TempDir::new().expect("tempdir");
    let repo = base.path().join("repo");
    write(&repo, "AGENTS.md", "# Root\n");
    write(&repo, "skipped/AGENTS.md", "# Skipped\n");
    let filler = "#".repeat((1 << 20) - "skipped/\n".len());
    let ignore_file = write(&repo, ".gitignore", &format!("skipped/\n{filler}"));

    assert_eq!(relative_roots(&repo), [PathBuf::from("AGENTS.md")]);

    fs::write(&ignore_file, format!("skipped/\n{filler}#")).expect("grow the ignore file");
    let refused = discover_context_roots(&repo);

    assert!(
        matches!(
            &refused,
            Err(LintError::IgnoreFileOversized { path, max_bytes: 1_048_576 }) if path == &ignore_file
        ),
        "{refused:?}"
    );
}
