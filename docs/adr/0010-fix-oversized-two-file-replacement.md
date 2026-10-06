# 0010: `fix_oversized` replaces two files with two renames, archive first

**Status:** Accepted. Recorded in 2026-10 from aoa-a03ih, the second review
round of aoa-4xshs. The review found that the previous change described the
replacement of the context file and its archive as a single step. It is two
renames, and no filesystem call makes two renames atomic together. The project
lead ruled on 2026-10-05 that the window between them is accepted and that what
holds inside it has to be stated and tested. Amended 2026-10-06 from aoa-rqnb8:
a run whose archive name is already taken is refused before anything is
written, which replaces the rerun convergence this record first promised.

## Context

`aoa_budget::fix_oversized` brings an over-budget context file under its
ceiling. It writes the full original body to a sibling `<stem>.archive.md` and
rewrites the context file as a summary that names the archive.

Each file is replaced by writing a temporary file beside it, giving that file
the mode, owner and attributes of the one it replaces, and renaming it into
place. One rename is atomic. The two renames are separate system calls, so a
failure or a kill can land between them.

## Decision

Both temporary files are prepared before either is placed, so every failure
that can be found early (a file that cannot be written, a directory that cannot
hold a new file) is found while nothing has changed.

The archive is placed first and the context file second. The order is the
decision: between the two renames the context file still holds the full
original body, and the archive holds the same body. No state in that window is
missing content.

The window itself is accepted. `fix_oversized` does not try to roll the archive
back when the context file cannot be placed, because a rollback is a third
rename with a window of its own.

A run that finds any entry at `<stem>.archive.md` is refused. It returns
`BudgetError::ArchiveExists`, which names the context file and the archive
path, and writes nothing. The rule looks at the directory entry only: a regular
file, a link of any kind, a directory or a link loop at that name all refuse
the run, and the content of the entry and the header of the context file are
never inspected (ruled 2026-10-06 in aoa-rqnb8). One rule covers the deliberate
second call, the rerun after a failure inside the window and the rerun after a
failure past it, and none of them can write a summary over the original.

## Consequences

Inside the window, meaning after the archive is placed and before the context
file is placed:

- A failure to place the context file returns an error and leaves the context
  file untouched. The archive already holds the original body. The same is true
  when the archive is in place and the sync of its directory then fails.
- Running `fix_oversized` again is refused, because the archive now stands.
  Both files hold the original body, so recovery is the operator's: confirm
  the archive against the context file, remove the archive, and run again. The
  convergence promised before aoa-rqnb8 (ruled 2026-10-05 in aoa-40h8p) is
  withdrawn; a rerun that overwrote the archive was the path by which a
  summary replaced the original.
- An archive that stood before the run refuses it, whatever the archive holds.
  `fix_oversized` never replaces an archive.

A temporary file is removed when its replacement is refused. When it can be
neither placed nor removed, the returned error is
`BudgetError::TempFileLeftBehind` and names the file. Both temporary files exist
from the moment the second is prepared until the archive is placed, so a
failure in that span withdraws the one not yet placed as well, and the error
names every file that could not be removed: a run whose archive is refused with
its temporary file stuck, and whose prepared context-file replacement is stuck
too, names both, in the order they were left.

What this record does not cover:

- A process killed between the two renames leaves the killed run's prepared
  temporary file for the context file behind. This is accepted as a residual
  and not fixed: the content converges with no duplication, the temporary
  names are unique so the leftover never changes a later result, and deleting
  files by pattern could remove a concurrent run's temporary file. Nothing
  removes it on the next run. An operator finds it in the context file's
  directory as `.aoa-budget-` followed by six random letters and digits, so
  the pattern `.aoa-budget-??????` matches it (the `tempfile` crate's default
  name length behind the prefix `replace.rs` sets). It is not linked from any
  context file, so it does not enter a closure.
- A run that fails after both files are placed (the sync of the context file's
  directory, or the recount that follows the write) returns an error with the
  summary already in place. A rerun is refused by the archive that stands, so
  the summary is never archived over the original; the operator reads the
  error, compares the two files and decides which to keep.
- The check for an existing archive and the rename that places one are
  separate calls. An entry created at the archive name between them is
  replaced by the rename. Nothing in this crate writes there concurrently, and
  the gap is accepted as it is for the two renames.

## Where this lives

- `crates/aoa-budget/src/fix.rs` holds the order: `fix_placing` refuses a
  taken archive name, prepares both replacements, then places the archive,
  then the context file.
- `crates/aoa-budget/src/replace.rs` holds one replacement. `Placing` is the
  crate-private pair of steps (rename, directory sync) that the tests fail one
  at a time; `fix_oversized` always passes the real pair.
- The unit tests in `crates/aoa-budget/src/fix.rs` fail the second rename and
  the first directory sync, check that both files hold the original body, and
  check that a rerun is refused with both files still holding it. The same
  file strands both temporary files and checks the error names both;
  `replace.rs` checks the order they are named in.
- `crates/aoa-budget/tests/budget.rs` runs the fix twice on one file and checks
  the second run is refused with the archive byte for byte the original. The
  same file and `tests/fix_replace.rs` put a file, a link inside and outside
  the boundary, a dangling link, a loop and a directory at the archive name and
  check each refuses the run and is left as it was.
