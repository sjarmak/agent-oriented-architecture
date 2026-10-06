# 0011: Windows keeps four filesystem checks that are checks, then uses

**Status:** Accepted. Recorded in 2026-10 from aoa-ea601, the Codex review of
aoa-rqnb8 and aoa-4xshs. The review found two places that re-resolved a path
after checking it and three places whose non-Unix arm never had the check at
all. The two were fixed on every platform by holding the directory or the file
open and acting through the handle. The three are fixed on Unix and recorded
here on Windows, under the precedent of
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
  followed: the temporary files and both renames land in the directory that
  was checked, and nothing is written where the link points
  (`a_directory_swapped_for_a_link_while_placing_writes_nothing_where_the_link_points`
  in `fix.rs`). The archive is placed with `linkat` then `unlinkat`, so an
  entry created at the archive name after the check is kept and the run is
  refused with `AlreadyExists`
  (`an_entry_made_at_the_archive_name_after_the_check_is_kept_and_nothing_is_replaced`).
  The non-Unix arm of the same type uses `std::fs` by path under the held
  directory's name. It cannot hold the directory, so the swap is not caught
  there.
- **`aoa-lint` reads ignore rules from the handle it validated.**
  `crates/aoa-lint/src/discover.rs` opens each `.gitignore` and `.ignore`
  through `aoa_budget::Enclosure`, which resolves the path inside the linted
  directory and opens the resolved member with `O_NOFOLLOW` on every step. The
  kind, link count and bytes come from that one handle, and the matcher is
  built from those bytes; no walker reopens the file by path. On Windows the
  open is `File::open` of the resolved path, so a swap between resolution and
  open is not caught, and the link-count refusal below does not apply.
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
- `crates/aoa-lint/src/discover.rs` holds the handle-validated ignore reader
  and `has_another_name`.
- `crates/aoa-path-trust/src/nofollow.rs` holds both arms of the trust-file
  open.
- `crates/aoa-audit/src/answer.rs` holds both arms of the deadline reader.
