use std::fs;
use std::path::{Path, PathBuf};

use aoa_budget::{
    resolve_closure, resolve_closure_within, resolve_contained_closure, BudgetError, UnreadLink,
    UnreadReason, MAX_CONTEXT_FILE_BYTES,
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

    assert_eq!(
        closure.outside_boundary,
        [
            dir.path().join("outside.md"),
            dir.path().join("absent.md"),
            dir.path().to_path_buf(),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_member_linking_outside_the_boundary_reads_the_same_whatever_it_reaches() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "repo/AGENTS.md", "[probe](probe.md)\n");
    std::os::unix::fs::symlink("../outside.md", dir.path().join("repo/probe.md")).unwrap();
    let outside = dir.path().join("outside.md");
    let reported = || {
        let closure = resolve_closure_within(&root, &dir.path().join("repo")).unwrap();
        (closure.outside_boundary, closure.unread)
    };

    let while_absent = reported();
    fs::write(&outside, "outside\n").unwrap();
    let while_a_file = reported();
    fs::remove_file(&outside).unwrap();
    fs::create_dir(&outside).unwrap();
    let while_a_directory = reported();

    assert_eq!(
        while_absent,
        (vec![dir.path().join("repo/probe.md")], Vec::new())
    );
    assert_eq!(while_a_file, while_absent);
    assert_eq!(while_a_directory, while_absent);
}

#[cfg(unix)]
fn across_what_is_outside<T>(outside: &Path, observe: impl Fn() -> T) -> [T; 4] {
    let while_absent = observe();
    fs::create_dir(outside).unwrap();
    let while_a_directory = observe();
    fs::remove_dir(outside).unwrap();
    fs::write(outside, "outside\n").unwrap();
    let while_a_file = observe();
    fs::remove_file(outside).unwrap();
    std::os::unix::fs::symlink("removed", outside).unwrap();
    let while_a_broken_link = observe();
    [
        while_absent,
        while_a_directory,
        while_a_file,
        while_a_broken_link,
    ]
}

#[cfg(unix)]
#[test]
fn a_member_linked_out_of_the_boundary_and_back_in_is_outside_whatever_it_steps_through() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    let root = write(dir.path(), "repo/AGENTS.md", "[probe](probe.md)\n");
    write(dir.path(), "repo/safe.md", "safe\n");
    std::os::unix::fs::symlink("../probed-dir/../repo/safe.md", repo.join("probe.md")).unwrap();

    let observed = across_what_is_outside(&dir.path().join("probed-dir"), || {
        let closure = resolve_closure_within(&root, &repo).unwrap();
        (
            member_names(&root, &repo),
            closure.outside_boundary,
            closure.unread,
        )
    });

    for seen in observed {
        assert_eq!(
            seen,
            (
                vec!["AGENTS.md".to_string()],
                vec![repo.join("probe.md")],
                Vec::new()
            )
        );
    }
}

#[cfg(unix)]
#[test]
fn a_link_that_steps_back_within_the_boundary_to_a_missing_file_is_a_broken_symlink() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    let root = write(dir.path(), "repo/AGENTS.md", "[probe](probe.md)\n");
    fs::create_dir(repo.join("docs")).unwrap();
    std::os::unix::fs::symlink("docs/../missing.md", repo.join("probe.md")).unwrap();

    let closure = resolve_closure_within(&root, &repo).unwrap();

    assert!(
        closure.outside_boundary.is_empty(),
        "{:?}",
        closure.outside_boundary
    );
    assert_eq!(
        closure.unread,
        [unread(repo.join("probe.md"), UnreadReason::BrokenSymlink)]
    );
}

#[cfg(unix)]
#[test]
fn a_link_that_steps_back_through_a_file_does_not_resolve() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[probe](probe.md)\n");
    write(dir.path(), "safe.md", "safe\n");
    std::os::unix::fs::symlink("AGENTS.md/../safe.md", dir.path().join("probe.md")).unwrap();

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(closure.files.len(), 1);
    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("probe.md"),
            UnreadReason::BrokenSymlink
        )]
    );
}

#[cfg(unix)]
#[test]
fn a_contained_root_linked_out_of_the_boundary_is_refused_whatever_it_reaches() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    std::os::unix::fs::symlink("../probed.md", repo.join("AGENTS.md")).unwrap();

    let observed = across_what_is_outside(&dir.path().join("probed.md"), || {
        resolve_contained_closure(&repo.join("AGENTS.md"), &repo)
            .unwrap_err()
            .to_string()
    });

    for seen in observed {
        assert!(seen.contains("resolves outside"), "{seen}");
    }
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
fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;

    let raw = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0, "mkfifo");
}

#[cfg(unix)]
fn unread(path: PathBuf, reason: UnreadReason) -> UnreadLink {
    UnreadLink { path, reason }
}

#[cfg(unix)]
#[test]
fn a_link_to_a_pipe_is_reported_and_a_directory_is_skipped_without_being_read() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[pipe](pipe.md) [dir](docs)\n");
    fs::create_dir(dir.path().join("docs")).unwrap();
    make_fifo(&dir.path().join("pipe.md"));

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(closure.files.len(), 1);
    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("pipe.md"),
            UnreadReason::NotRegularFile
        )]
    );
}

#[cfg(unix)]
#[test]
fn a_link_to_a_socket_is_reported_as_not_a_regular_file() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[socket](socket.md)\n");
    let _listener = std::os::unix::net::UnixListener::bind(dir.path().join("socket.md")).unwrap();

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(closure.files.len(), 1);
    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("socket.md"),
            UnreadReason::NotRegularFile
        )]
    );
}

#[cfg(unix)]
#[test]
fn a_device_inside_the_boundary_is_refused_by_its_file_type() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[zero](zero.md)\n");
    std::os::unix::fs::symlink("/dev/zero", dir.path().join("zero.md")).unwrap();

    let closure = resolve_closure_within(&root, Path::new("/")).unwrap();

    assert_eq!(closure.files.len(), 1);
    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("zero.md"),
            UnreadReason::NotRegularFile
        )]
    );
    assert!(closure.outside_boundary.is_empty());
}

#[cfg(unix)]
#[test]
fn a_member_that_cannot_be_counted_is_reported_with_the_reason() {
    let dir = TempDir::new().unwrap();
    let root = write(
        dir.path(),
        "AGENTS.md",
        "[gone](gone.md) [loop](loop.md) [binary](binary.md) [absent](absent.md) @mention\n",
    );
    std::os::unix::fs::symlink("removed.md", dir.path().join("gone.md")).unwrap();
    std::os::unix::fs::symlink("loop.md", dir.path().join("loop.md")).unwrap();
    fs::write(dir.path().join("binary.md"), b"rules\n\xff\xfe\n").unwrap();

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(closure.files.len(), 1);
    assert_eq!(
        closure.unread,
        [
            unread(dir.path().join("gone.md"), UnreadReason::BrokenSymlink),
            unread(dir.path().join("loop.md"), UnreadReason::BrokenSymlink),
            unread(dir.path().join("binary.md"), UnreadReason::NotUtf8),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_broken_symlink_outside_the_boundary_is_named_as_outside_and_not_as_unread() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "repo/AGENTS.md", "[gone](../gone.md)\n");
    std::os::unix::fs::symlink("removed.md", dir.path().join("gone.md")).unwrap();

    let closure = resolve_closure_within(&root, &dir.path().join("repo")).unwrap();

    assert!(closure.unread.is_empty(), "{:?}", closure.unread);
    assert_eq!(closure.outside_boundary, [dir.path().join("gone.md")]);
}

#[cfg(unix)]
#[test]
fn a_broken_symlink_reached_through_a_directory_alias_is_reported_once() {
    let dir = TempDir::new().unwrap();
    let root = write(
        dir.path(),
        "AGENTS.md",
        "[direct](real/gone.md) [aliased](alias/gone.md)\n",
    );
    fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink("removed.md", dir.path().join("real/gone.md")).unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("alias")).unwrap();

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("real/gone.md"),
            UnreadReason::BrokenSymlink
        )]
    );
}

#[cfg(unix)]
#[test]
fn a_member_the_process_may_not_open_is_reported_as_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[sealed](sealed.md)\n");
    let sealed = write(dir.path(), "sealed.md", "rules\n");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&sealed).is_ok() {
        eprintln!(
            "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process opens a \
             mode-000 file, so it cannot be denied a read"
        );
        return;
    }

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(closure.files.len(), 1);
    assert_eq!(closure.unread, [unread(sealed, UnreadReason::Unreadable)]);
}

#[cfg(unix)]
#[test]
fn a_link_beneath_a_directory_the_process_may_not_search_is_unreadable_not_absent() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[sealed](sealed/rules.md)\n");
    let rules = write(dir.path(), "sealed/rules.md", "rules\n");
    let sealed = dir.path().join("sealed");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();
    let searchable = fs::symlink_metadata(&rules).is_ok();

    let closure = resolve_closure_within(&root, dir.path());

    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).unwrap();
    if searchable {
        eprintln!(
            "SKIP (docs/adr/0004-environment-dependent-test-skips.md): this process searches a \
             mode-000 directory, so it cannot be denied a lookup"
        );
        return;
    }
    let closure = closure.unwrap();
    assert_eq!(closure.files.len(), 1);
    assert_eq!(closure.unread, [unread(rules, UnreadReason::Unreadable)]);
    assert!(closure.absent.is_empty(), "{:?}", closure.absent);
}

#[test]
fn a_member_of_exactly_the_size_limit_is_counted() {
    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[big](big.md)\n");
    let big = fs::File::create(dir.path().join("big.md")).unwrap();
    big.set_len(MAX_CONTEXT_FILE_BYTES).unwrap();

    let closure = resolve_closure(&root).unwrap();

    assert_eq!(closure.files.len(), 2);
    assert_eq!(closure.files[1].text.len() as u64, MAX_CONTEXT_FILE_BYTES);
    assert!(closure.unread.is_empty());
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

#[cfg(unix)]
const SWAP_RACE_ROUNDS: usize = 20_000;

#[cfg(unix)]
fn leaks_while_a_directory_is_swapped_for_a_symlink(
    scratch: &Path,
    directory: &Path,
    resolves_outside_text: impl Fn() -> bool,
) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};

    let spare = scratch.join("repo/spare");
    let held = scratch.join("held");
    std::os::unix::fs::symlink("../outside", &spare).unwrap();
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                fs::rename(directory, &held).unwrap();
                fs::rename(&spare, directory).unwrap();
                fs::rename(directory, &spare).unwrap();
                fs::rename(&held, directory).unwrap();
            }
        });
        let leaked = (0..SWAP_RACE_ROUNDS).any(|_| resolves_outside_text());
        stop.store(true, Ordering::Relaxed);
        leaked
    })
}

#[cfg(unix)]
#[test]
fn a_member_whose_directory_is_swapped_for_a_symlink_is_never_read_from_outside() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    write(dir.path(), "outside/member.md", "outside the repository\n");
    let root = write(dir.path(), "repo/AGENTS.md", "[member](docs/member.md)\n");
    write(dir.path(), "repo/docs/member.md", "inside\n");

    let leaked =
        leaks_while_a_directory_is_swapped_for_a_symlink(dir.path(), &repo.join("docs"), || {
            resolve_closure_within(&root, &repo)
                .unwrap()
                .files
                .iter()
                .any(|file| file.text.contains("outside"))
        });

    assert!(!leaked, "a member was read from outside the boundary");
}

#[cfg(unix)]
#[test]
fn a_contained_root_whose_directory_is_swapped_for_a_symlink_is_never_read_from_outside() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    write(dir.path(), "outside/AGENTS.md", "outside the repository\n");
    let root = write(dir.path(), "repo/docs/AGENTS.md", "inside\n");

    let leaked =
        leaks_while_a_directory_is_swapped_for_a_symlink(dir.path(), &repo.join("docs"), || {
            resolve_contained_closure(&root, &repo).is_ok_and(|closure| {
                closure
                    .files
                    .iter()
                    .any(|file| file.text.contains("outside"))
            })
        });

    assert!(!leaked, "the root was read from outside the boundary");
}

#[cfg(target_os = "linux")]
#[test]
fn a_member_under_a_directory_that_may_only_be_searched_is_counted() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let root = write(dir.path(), "AGENTS.md", "[member](sealed/member.md)\n");
    write(dir.path(), "sealed/member.md", "rules\n");
    let sealed = dir.path().join("sealed");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o111)).unwrap();

    let names = member_names(&root, dir.path());
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(names, ["AGENTS.md", "sealed/member.md"]);
}

#[cfg(unix)]
#[test]
fn a_link_through_an_unresolvable_directory_is_reported_when_the_root_is_named_by_an_alias() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "real/AGENTS.md", "[lost](loop/lost.md)\n");
    std::os::unix::fs::symlink("loop", dir.path().join("real/loop")).unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("alias")).unwrap();

    let closure = resolve_closure_within(
        &dir.path().join("alias/AGENTS.md"),
        &dir.path().join("real"),
    )
    .unwrap();

    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("alias/loop/lost.md"),
            UnreadReason::Unreadable
        )]
    );
}

#[cfg(unix)]
#[test]
fn a_link_through_an_unresolvable_directory_is_reported_once_across_aliases() {
    let dir = TempDir::new().unwrap();
    let root = write(
        dir.path(),
        "AGENTS.md",
        "[aliased](alias/loop/lost.md) [direct](loop/lost.md)\n",
    );
    std::os::unix::fs::symlink("loop", dir.path().join("loop")).unwrap();
    std::os::unix::fs::symlink(".", dir.path().join("alias")).unwrap();

    let closure = resolve_closure_within(&root, dir.path()).unwrap();

    assert_eq!(
        closure.unread,
        [unread(
            dir.path().join("alias/loop/lost.md"),
            UnreadReason::Unreadable
        )]
    );
}
