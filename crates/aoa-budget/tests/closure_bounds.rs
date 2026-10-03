use std::fs;
use std::path::{Path, PathBuf};

use aoa_budget::{
    resolve_closure, resolve_closure_within, resolve_contained_closure, BudgetError,
    MAX_CONTEXT_FILE_BYTES,
};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

fn member_names(root: &Path, boundary: &Path) -> Vec<String> {
    resolve_closure_within(root, boundary)
        .unwrap()
        .files
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(boundary)
                .unwrap_or(&file.path)
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn a_link_leaving_the_boundary_is_not_a_closure_member() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "outside.md", "outside the repository\n");
    let root = write(
        dir.path(),
        "repo/AGENTS.md",
        "[out](../outside.md) [in](docs/rules.md)\n",
    );
    write(dir.path(), "repo/docs/rules.md", "rules\n");

    let names = member_names(&root, &dir.path().join("repo"));

    assert_eq!(names, ["AGENTS.md", "docs/rules.md"]);
}

#[test]
fn a_link_leaving_the_boundary_is_named_by_the_path_that_was_written() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "outside.md", "outside the repository\n");
    let root = write(
        dir.path(),
        "repo/AGENTS.md",
        "[out](../outside.md) [gone](../absent.md) [up](..)\n",
    );

    let closure = resolve_closure_within(&root, &dir.path().join("repo")).unwrap();

    assert_eq!(closure.outside_boundary, [dir.path().join("outside.md")]);
}

#[cfg(unix)]
#[test]
fn a_file_reached_through_directory_aliases_is_a_member_once() {
    let dir = TempDir::new().unwrap();
    let hops = |prefix: &str| {
        format!("[a]({prefix}s/AGENTS.md) [b]({prefix}t/AGENTS.md) [c]({prefix}s/t/more.md)\n")
    };
    let root = write(dir.path(), "AGENTS.md", &hops(""));
    write(dir.path(), "more.md", &hops("s/t/"));
    std::os::unix::fs::symlink(".", dir.path().join("s")).unwrap();
    std::os::unix::fs::symlink(".", dir.path().join("t")).unwrap();

    assert_eq!(
        member_names(&root, dir.path()),
        ["AGENTS.md", "s/t/more.md"]
    );
}

#[cfg(unix)]
#[test]
fn a_contained_closure_refuses_a_root_that_resolves_outside_the_boundary() {
    let dir = TempDir::new().unwrap();
    let secret = write(dir.path(), "secret.md", "outside the repository\n");
    fs::create_dir(dir.path().join("repo")).unwrap();
    let root = dir.path().join("repo/AGENTS.md");
    std::os::unix::fs::symlink(&secret, &root).unwrap();

    let err = resolve_contained_closure(&root, &dir.path().join("repo")).unwrap_err();

    assert!(matches!(err, BudgetError::OutsideBoundary { .. }), "{err}");
    assert_eq!(
        resolve_closure_within(&root, &dir.path().join("repo"))
            .unwrap()
            .files
            .len(),
        1
    );
}

#[cfg(unix)]
#[test]
fn a_contained_closure_checks_the_root_it_goes_on_to_read() {
    let dir = TempDir::new().unwrap();
    let secret = write(dir.path(), "secret.md", "outside the repository\n");
    write(dir.path(), "repo/inside/AGENTS.md", "contained\n");
    fs::create_dir_all(dir.path().join("repo/inside/deep")).unwrap();
    fs::create_dir_all(dir.path().join("repo/sub")).unwrap();
    std::os::unix::fs::symlink("../inside/deep", dir.path().join("repo/sub/link")).unwrap();
    std::os::unix::fs::symlink(&secret, dir.path().join("repo/sub/AGENTS.md")).unwrap();

    let err = resolve_contained_closure(
        &dir.path().join("repo/sub/link/../AGENTS.md"),
        &dir.path().join("repo"),
    )
    .unwrap_err();

    assert!(matches!(err, BudgetError::OutsideBoundary { .. }), "{err}");
}

#[cfg(unix)]
#[test]
fn file_aliases_stay_separate_members_and_expand_from_their_own_directory() {
    let dir = TempDir::new().unwrap();
    let root = write(
        dir.path(),
        "AGENTS.md",
        "[a](a/doc.md) [b](b/doc.md) [alias](alias.txt) [real](shared.md)\n",
    );
    write(dir.path(), "shared.md", "[child](child.md)\n");
    write(dir.path(), "a/child.md", "small\n");
    write(dir.path(), "b/child.md", "large\n");
    std::os::unix::fs::symlink("../shared.md", dir.path().join("a/doc.md")).unwrap();
    std::os::unix::fs::symlink("../shared.md", dir.path().join("b/doc.md")).unwrap();
    std::os::unix::fs::symlink("shared.md", dir.path().join("alias.txt")).unwrap();

    assert_eq!(
        member_names(&root, dir.path()),
        [
            "AGENTS.md",
            "a/doc.md",
            "a/child.md",
            "b/doc.md",
            "b/child.md",
            "alias.txt",
            "shared.md",
        ]
    );
}

#[test]
fn a_nested_root_reaches_a_sibling_directory_inside_the_boundary() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "docs/shared.md", "shared\n");
    let root = write(dir.path(), "pkg/AGENTS.md", "[shared](../docs/shared.md)\n");

    assert_eq!(
        member_names(&root, dir.path()),
        ["pkg/AGENTS.md", "docs/shared.md"]
    );
    assert_eq!(resolve_closure(&root).unwrap().files.len(), 1);
}

#[cfg(unix)]
#[test]
fn a_linked_symlink_counts_only_when_its_target_is_inside_the_boundary() {
    let dir = TempDir::new().unwrap();
    let secret = write(dir.path(), "secret.md", "outside the repository\n");
    let root = write(
        dir.path(),
        "repo/AGENTS.md",
        "[escape](escape.md) [alias](alias.md)\n",
    );
    write(dir.path(), "repo/docs/rules.md", "rules\n");
    std::os::unix::fs::symlink(&secret, dir.path().join("repo/escape.md")).unwrap();
    std::os::unix::fs::symlink("docs/rules.md", dir.path().join("repo/alias.md")).unwrap();

    let names = member_names(&root, &dir.path().join("repo"));

    assert_eq!(names, ["AGENTS.md", "alias.md"]);
}

#[cfg(unix)]
#[test]
fn a_link_to_a_device_or_a_directory_is_skipped_without_being_read() {
    let dir = TempDir::new().unwrap();
    let root = write(
        dir.path(),
        "AGENTS.md",
        "[zero](zero.md) [pipe](pipe.md) [dir](docs)\n",
    );
    fs::create_dir(dir.path().join("docs")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", dir.path().join("zero.md")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(dir.path().join("pipe.md"))
        .status()
        .expect("mkfifo available");
    assert!(made.success());

    assert_eq!(member_names(&root, dir.path()), ["AGENTS.md"]);
}

#[test]
fn an_oversized_member_fails_the_closure_by_name() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[big](big.md)\n");
    let big = fs::File::create(dir.path().join("big.md")).unwrap();
    big.set_len(MAX_CONTEXT_FILE_BYTES + 1).unwrap();

    let err = resolve_closure(&root).unwrap_err();

    let BudgetError::Oversized { path, max_bytes } = err else {
        panic!("expected an oversized error, got {err}");
    };
    assert!(path.ends_with("big.md"), "{}", path.display());
    assert_eq!(max_bytes, MAX_CONTEXT_FILE_BYTES);
}

#[test]
fn a_root_that_is_not_a_regular_file_is_an_error() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("AGENTS.md")).unwrap();

    let err = resolve_closure(&dir.path().join("AGENTS.md")).unwrap_err();

    assert!(matches!(err, BudgetError::Io { .. }), "{err}");
}
