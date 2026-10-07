# 0011: Windows keeps five sites by path where Unix acts through a held handle

**Status:** Accepted. Recorded in 2026-10 from aoa-ea601, the Codex review of
aoa-rqnb8 and aoa-4xshs. The review found two places that re-resolved a path
after checking it and three places whose non-Unix arm never had the check at
all. The two are fixed on Unix by holding the directory or the file open and
acting through the handle; their Windows arms still act by path and are the
first two sites below. The three are fixed on Unix and recorded here on
Windows, under the precedent of
[0007](0007-git-environment-and-config-for-subprocesses.md): CI never runs
Windows, so a Windows fix would ship unobserved.

## Context

The crates that write into or read from a checkout open paths twice: once to
decide whether the path is safe to act on and once to act. Between the two a
directory on the path can be renamed aside and replaced with a link, which
redirects the second open to wherever the link points. On Unix the fix is a
descriptor-relative call (`openat`, `renameat`, `linkat`, `fstat`) against a
directory that was opened with `O_NOFOLLOW` on every step and is held for the
whole operation. Windows has `NtCreateFile` with a root handle, but the
standard library does not expose it, no crate in the workspace wraps it, and
nothing in CI would run the result.

## Decision

Unix is correct and tested. Windows keeps a by-path arm at each site below,
and this record is where that is stated. A Windows fix overturns this record
when CI gains a Windows runner, not before.

The sites, each with what its Unix arm now guarantees and what the Windows arm
does not:

- **`aoa-budget` places both files relative to a held directory.**
  `crates/aoa-budget/src/directory.rs` holds the directory that was resolved
  inside the boundary and creates, renames, links, stats and syncs through its
  descriptor. A directory swapped for a link after it was held is not
  followed: the temporary files and both placements land in the directory
  that was checked, the recount reads the context file through the same
  descriptor, and nothing is written where the link points
  (`a_directory_swapped_for_a_link_while_placing_writes_nothing_where_the_link_points`
  in `fix.rs`). The archive is placed with `renameat2` and `RENAME_NOREPLACE`
  on Linux and with `linkat` then `unlinkat` on other Unix, so an entry created
  at the archive name after the check is kept and the run is refused with
  `AlreadyExists`
  (`an_entry_made_at_the_archive_name_after_the_check_is_kept_and_nothing_is_replaced`).
  `Boundary::reach` in `boundary.rs` decides whether a linked file exists and
  where it resolves by walking each step of its path with `statat` and
  `readlinkat` relative to a directory descended from the held boundary
  descriptor, the same descent `open_member` then reads through. When the
  walk ends without a member, `Boundary::kind_named` decides what the closure
  says about the link, a broken symlink, an unreadable entry or nothing, with
  the same descent and a no-follow `statat` of the final name in the
  directory the walk reached. A boundary swapped for a link after it was held
  therefore resolves, reads and classifies what was held and consults the
  replacement for none of it
  (`a_linked_file_the_held_directory_has_is_read_after_the_directory_is_swapped_for_a_link_lacking_it`,
  `a_linked_file_only_the_replacement_has_is_absent_after_the_directory_is_swapped_for_a_link`
  and
  `a_link_only_the_replacement_has_is_absent_not_broken_after_the_directory_is_swapped_for_a_link`
  in `tests/closure_bounds.rs`). The non-Unix arm of the same type uses
  `std::fs` by path under the held directory's name, and its `reach` and
  `kind_named` walk with `symlink_metadata` and `read_link` from the
  canonical name. It cannot hold the directory, so the swap is not caught
  there.
- **`aoa-budget` needs read permission on every directory it descends on
  Unix other than Linux and Android.** The descent above opens each directory
  with `O_PATH | O_DIRECTORY` on Linux and Android and with
  `O_RDONLY | O_DIRECTORY` elsewhere (`descend::TRAVERSE` in `boundary.rs`),
  because only Linux has `O_PATH`. The pathname lookup the descent replaced
  needed search permission on a directory it passed through; `O_RDONLY` needs
  read permission on it. On those systems an execute-only directory (mode
  `0111`) beneath the boundary makes `reach` fail with `PermissionDenied`, so
  a linked file under it is reported unreadable where the pathname walk would
  have read it or reported it absent, and a link whose target steps through
  such a directory with `..` fails the same way. CI runs Linux only and every
  read of a member already goes through the same descent, so the code stays
  and this record is where the difference is stated. The Linux arm is the
  specification; a port that gains an `O_PATH` equivalent overturns this
  bullet.
- **`aoa-lint` walks, reads ignore rules and reads discovered roots through
  held directories.** `crates/aoa-lint/src/discover.rs` holds the linted
  directory through `aoa_budget::Enclosure::open`, lists each directory from
  its descriptor (`Directory::entries`) and steps into a child with
  `openat(O_DIRECTORY | O_NOFOLLOW)` on that descriptor (`Directory::descend`),
  so a directory swapped for a link after it was listed is never entered and
  never re-listed by path. Each `.gitignore` and `.ignore` is opened with
  `O_NOFOLLOW` relative to the held directory (`Directory::open_entry`); only
  an entry that is itself a link is resolved by path, through the boundary,
  and a link that leaves the linted directory is refused. The kind, link count
  and bytes come from that one handle, and the matcher is built from those
  bytes. A directory swapped for a link after its rules were checked lists
  what was held, not what the link reaches
  (`a_directory_swapped_for_a_link_after_its_rules_were_checked_lists_what_was_held_not_what_the_link_reaches`
  in `discover.rs`). Discovery and reading hang off one `LintedDirectory`,
  which holds the linted directory once and keeps each discovered root
  together with the held directory it was listed in and its entry name. The
  root is then read with `Enclosure::resolve_held`, which opens it relative to
  that descriptor with `O_NOFOLLOW` and walks its closure under the boundary
  discovery held, so neither the linted directory nor the root's directory is
  reopened by path between discovery and reading. Swapping the linted
  directory itself for a link in that window reads what was held and lints
  nothing the link reaches
  (`the_linted_directory_swapped_for_a_link_between_discovery_and_reading_is_read_as_discovered`
  in `tests/discovery.rs`), and swapping the root's own directory does the
  same
  (`a_discovered_root_whose_directory_became_a_link_is_read_as_held_while_the_same_root_named_follows_the_link`).
  A discovered root therefore no longer raises `OutsideBoundary`; a root the
  caller named is still read as named, links followed. Every name handed to
  `Directory::open_entry`, `open_member` or `descend` must be one normal path
  component, so a name carrying a separator, `.`, `..` or an absolute path is
  refused as `NotAnEntryName` before anything opens
  (`a_name_that_is_not_one_entry_is_refused_before_anything_opens` in
  `directory.rs`); a trailing separator would otherwise make `O_NOFOLLOW`
  follow a directory link. On Windows `Directory::entries` is
  `std::fs::read_dir` of the held name and `Directory::descend` reopens the
  child by path, entering a link to a directory. `Directory::open_entry`,
  which opens an ignore file, is `symlink_metadata` then `File::open` of the
  joined path: an entry that is a link when checked is resolved by path
  through the boundary as above, but one replaced with a link between the two
  calls is followed. `Directory::open_member`, which reads a discovered root
  for `Enclosure::resolve_held`, is `File::open` of the joined path with no
  check before it, so a root replaced with a link after discovery is read from
  wherever the link points. No swap between any of those steps is caught, and
  the link-count refusal below does not apply.
- **`aoa-lint` cannot see a second name on Windows.** `has_another_name` reads
  the Unix link count. The Windows arm returns `false`, so an ignore file that
  is a hard link to a file outside the linted directory is read there. Rust's
  `std::os::windows::fs::MetadataExt::number_of_links` exists but is unstable,
  and nothing would run a test of it.
- **`aoa-path-trust` opens the trust file by path on Windows.**
  `read_regular_file_nofollow` in `crates/aoa-path-trust/src/nofollow.rs`
  opens the named entry of a held directory with `O_NOFOLLOW` on Unix and
  checks the kind of what it opened. The non-Unix arm reads `symlink_metadata`
  by path, refuses a link or a non-file, and then calls `File::open` on the
  same path: an entry replaced between the two calls is the one that is read.
- **`aoa-audit` can leave a reader thread blocked on Windows.**
  `Answering` in `crates/aoa-audit/src/answer.rs` reads a git subprocess's
  output under a deadline. The Unix arm sets the pipe non-blocking and polls
  it, so after the deadline the pipe is simply dropped. The non-Unix arm reads
  to end on a detached thread and gives up waiting for that thread at the
  deadline; if the killed git left a descendant holding the write end of the
  pipe, the thread stays blocked until that descendant exits. The audit's
  answer is unaffected, the thread is one per timed-out call, and the process
  exits normally without joining it.

### Case-folding volumes

The write gate compares path spellings, not files. `scope_under` in
`crates/aoa/src/commands/enforce/scope.rs` resolves a hook's write target twice,
lexically through `resolve_written_path` in
`crates/aoa-path-trust/src/resolve.rs` and through `resolve_canonicalizing`,
and `contained` strips the repository root from each to get the
repository-relative string the policy's globs are matched against. Both
resolutions keep the component spelling the hook payload used, or whatever
`realpath` returns, and neither is defined to return the spelling the volume
stores. On a volume that folds case (APFS and NTFS by default, an ext4
directory with the casefold attribute) `PROTECTED/new` and `protected/new`
name one entry, so a policy that protects `protected/**` is matched against
the payload's spelling and misses the write while it lands in the protected
directory. Byte-wise comparison is correct on every volume CI runs on, which is
case-sensitive ext4 only.

The site is recorded here rather than fixed because the fix waits on the same
event as the sites above: a runner with a case-folding volume, on which a test
can plant the two spellings and the gate can either compare the matched
ancestor by device and inode or look up the stored spelling. Until then no
test can prove either behaviour, and a check that is never run proves nothing
(aoa-7pkom item 2).

## Consequences

- A review that finds one of these arms is looking at a known residual, not a
  new finding. The fix for all of them is the same event: a Windows CI runner,
  after which each site is ported and this record is superseded.
- No `#[cfg(windows)]` test exists for any site, by
  [0004](0004-environment-dependent-test-skips.md): a test that can never run in
  CI is `#[ignore]`d or absent, and a check that is never run proves nothing.
  `cargo check --target x86_64-pc-windows-gnu` compiles the arms; that is the
  extent of their verification.
- The Unix arms are the specification. A Windows port must pass the tests
  named above against Windows semantics, not reproduce the by-path arm with a
  wider type.

## Where this lives

- `crates/aoa-budget/src/directory.rs`, `boundary.rs`, `replace.rs` and
  `fix.rs` hold the Unix descriptor-relative placement and its non-Unix arm.
- `crates/aoa-lint/src/discover.rs` holds the held-directory walk, the
  handle-validated ignore reader and `has_another_name`.
- `crates/aoa-path-trust/src/nofollow.rs` holds both arms of the trust-file
  open.
- `crates/aoa-audit/src/answer.rs` holds both arms of the deadline reader.
- `crates/aoa/src/commands/enforce/scope.rs` (`scope_under`, `contained`) and
  `crates/aoa-path-trust/src/resolve.rs` (`resolve_written_path`) hold the
  spelling comparison the case-folding residual is about.
